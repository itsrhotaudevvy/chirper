//! The Fluxer REST client: one place for the `/v1` base, the auth header, the 429 policy, bounded
//! bodies and the multipart upload format.
//!
//! Port of `fluxerapi/client.go`, `endpoints.go`, `auth.go` and `credential.go`, without the
//! ALTCHA solver (see `captcha.rs`) and without the calls that only the bridge connector used.

use std::fmt;
use std::time::Duration;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use url::Url;
use zeroize::Zeroizing;

use chirper_bridge::{Error, Result};

use crate::captcha::with_captcha;
use crate::discovery::Endpoints;
use crate::map::{Message, User};

/// Sent with every request, as required for well-behaved third-party clients.
pub const USER_AGENT: &str = concat!(
    "Chirper (https://chirper.app, ",
    env!("CARGO_PKG_VERSION"),
    ")"
);

/// Bounds every JSON response body.
pub const MAX_JSON_BYTES: usize = 8 << 20;
/// Bounds one downloaded attachment.
pub const MAX_MEDIA_BYTES: usize = 100 << 20;

const MAX_RATE_LIMIT_RETRIES: u32 = 3;
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);
const MAX_CODE_CHARS: usize = 64;

/// Everything outside the unreserved set is escaped, so an emoji or an id can never change the path.
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// The token that authenticates a client. A bot token is sent with the `Bot ` prefix.
#[derive(Clone)]
pub struct Credential {
    token: Zeroizing<String>,
    bot: bool,
}

impl Credential {
    pub fn new(token: String, bot: bool) -> Self {
        Self {
            token: Zeroizing::new(token),
            bot,
        }
    }

    /// No credential: used for the sign-in endpoints.
    pub fn anonymous() -> Self {
        Self::new(String::new(), false)
    }

    pub fn is_bot(&self) -> bool {
        self.bot
    }

    /// The raw token, for the Gateway Identify and Resume payloads. Never log it.
    pub fn token(&self) -> &str {
        self.token.as_str()
    }

    pub fn has_token(&self) -> bool {
        !self.token.is_empty()
    }

    /// The `Authorization` header value.
    fn authorization(&self) -> String {
        if self.bot {
            format!("Bot {}", self.token.as_str())
        } else {
            self.token.as_str().to_owned()
        }
    }
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = if self.bot { "bot" } else { "user" };
        write!(f, "Credential{{{kind}, token:[redacted]}}")
    }
}

/// A non-2xx answer from Fluxer. It never carries the request's credential.
#[derive(Debug, Default)]
pub struct ApiError {
    pub status: u16,
    pub code: String,
    /// Set for 429 responses.
    pub retry_after: Duration,
    /// True when the global bucket denied the request.
    pub global: bool,
    pub captcha_provider: String,
    /// The raw `altcha_challenge` object, kept verbatim for the solver.
    pub captcha_challenge: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ErrorBody {
    code: String,
    global: bool,
    retry_after: f64,
    captcha_provider: String,
    altcha_challenge: Option<Box<RawValue>>,
}

impl ApiError {
    /// Parse an error response. `retry_after_header` is the integer `Retry-After` header, used
    /// when the body does not carry `retry_after`.
    pub fn from_response(status: u16, retry_after_header: Option<u64>, body: &[u8]) -> Self {
        let parsed: ErrorBody = serde_json::from_slice(body).unwrap_or_default();
        let mut retry_after = secs_to_duration(parsed.retry_after);
        if let Some(secs) = retry_after_header.filter(|_| retry_after.is_zero()) {
            retry_after = Duration::from_secs(secs.min(86_400));
        }
        Self {
            status,
            code: parsed.code.chars().take(MAX_CODE_CHARS).collect(),
            retry_after,
            global: parsed.global,
            captcha_provider: parsed.captcha_provider,
            captcha_challenge: parsed.altcha_challenge.map(|raw| raw.get().to_owned()),
        }
    }

    /// Whether the server wants a solved ALTCHA challenge before it accepts the request.
    pub fn is_captcha_required(&self) -> bool {
        (self.code == "CAPTCHA_REQUIRED" || self.code == "INVALID_CAPTCHA")
            && self.captcha_provider == "altcha"
            && self.captcha_challenge.is_some()
    }

    pub fn has_code(&self, code: &str) -> bool {
        self.code == code
    }

    /// A short description for error text: the status and the code, never the body.
    pub fn describe(&self) -> String {
        if self.code.is_empty() {
            format!("HTTP {}", self.status)
        } else {
            format!("HTTP {} {}", self.status, self.code)
        }
    }
}

fn secs_to_duration(secs: f64) -> Duration {
    if secs.is_finite() && secs > 0.0 {
        Duration::from_secs_f64(secs.min(86_400.0))
    } else {
        Duration::ZERO
    }
}

impl From<ApiError> for Error {
    fn from(err: ApiError) -> Self {
        match err.status {
            401 => Error::Auth("Fluxer refused the token".to_owned()),
            429 => Error::RateLimited {
                retry_after_ms: u64::try_from(err.retry_after.as_millis()).unwrap_or(u64::MAX),
            },
            500..=599 => Error::Network(format!("Fluxer answered HTTP {}", err.status)),
            _ => Error::Protocol(format!("Fluxer answered {}", err.describe())),
        }
    }
}

/// A failed call: either the server answered with an error, or the call never got that far.
#[derive(Debug)]
pub enum CallErr {
    Api(ApiError),
    Other(Error),
}

impl From<Error> for CallErr {
    fn from(err: Error) -> Self {
        Self::Other(err)
    }
}

impl From<CallErr> for Error {
    fn from(err: CallErr) -> Self {
        match err {
            CallErr::Api(api) => api.into(),
            CallErr::Other(other) => other,
        }
    }
}

/// Map a transport failure. The request URL is dropped first: media URLs carry signatures.
pub fn network_error(err: reqwest::Error) -> Error {
    let err = err.without_url();
    let text = err.to_string();
    if err.is_timeout() || err.is_connect() || err.is_request() || err.is_body() {
        Error::Network(text)
    } else {
        Error::Protocol(text)
    }
}

/// Read a response body, refusing anything above `limit` bytes. The body is streamed, so a server
/// cannot make us hold more than the limit.
pub async fn read_limited(response: &mut reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|len| len > limit as u64)
    {
        return Err(too_large());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network_error)? {
        if body.len() + chunk.len() > limit {
            return Err(too_large());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn too_large() -> Error {
    Error::Protocol("Fluxer sent more data than the limit allows".to_owned())
}

/// Decide whether a failed call is retried. Only a 429 is, up to three times, when the wait is at
/// most a minute. A global denial of a user session token is never retried: the server revokes
/// that token (docs: topics/rate-limits.md).
pub fn retry_delay(err: &ApiError, attempt: u32, bot: bool) -> Option<Duration> {
    if err.status != 429 || attempt >= MAX_RATE_LIMIT_RETRIES {
        return None;
    }
    if err.global && !bot {
        return None;
    }
    if err.retry_after > MAX_RETRY_AFTER {
        return None;
    }
    Some(err.retry_after.max(Duration::from_secs(1)))
}

/// A request body and its media type.
#[derive(Clone)]
struct Body {
    content_type: String,
    bytes: Vec<u8>,
}

/// One REST call, built before it is sent. A call is re-sent unchanged after a 429.
pub struct Call {
    method: reqwest::Method,
    path: String,
    query: Vec<(&'static str, String)>,
    body: Option<Body>,
    captcha: Option<String>,
    anonymous: bool,
}

impl Call {
    fn new(method: reqwest::Method, path: String) -> Self {
        Self {
            method,
            path,
            query: Vec::new(),
            body: None,
            captcha: None,
            anonymous: false,
        }
    }

    fn get(path: String) -> Self {
        Self::new(reqwest::Method::GET, path)
    }

    fn post(path: String) -> Self {
        Self::new(reqwest::Method::POST, path)
    }

    fn json<T: Serialize + ?Sized>(mut self, value: &T) -> Result<Self> {
        let bytes = serde_json::to_vec(value)
            .map_err(|e| Error::Internal(format!("encoding a request: {e}")))?;
        self.body = Some(Body {
            content_type: "application/json".to_owned(),
            bytes,
        });
        Ok(self)
    }

    fn query(mut self, key: &'static str, value: String) -> Self {
        self.query.push((key, value));
        self
    }

    fn anonymous(mut self) -> Self {
        self.anonymous = true;
        self
    }

    fn captcha(mut self, token: Option<String>) -> Self {
        self.captcha = token;
        self
    }
}

/// Query and path bounds for the message list.
#[derive(Clone, Copy, Debug, Default)]
pub struct MessageQuery<'a> {
    pub limit: usize,
    pub before: Option<&'a str>,
}

/// A file sent as `files[id]` in a multipart message.
#[derive(Clone, Debug)]
pub struct Upload {
    pub id: u32,
    pub filename: String,
    pub content_type: String,
    pub data: Vec<u8>,
}

/// A message reference: the reply target.
#[derive(Clone, Debug, Default, Serialize)]
pub struct MessageRef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
}

/// The JSON of a create-message call. The attachments field is filled from the uploads.
#[derive(Clone, Debug, Default, Serialize)]
pub struct MessageBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_reference: Option<MessageRef>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<AttachmentMeta>,
}

/// The metadata of one uploaded file.
#[derive(Clone, Debug, Serialize)]
pub struct AttachmentMeta {
    pub id: u32,
    pub filename: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub content_type: String,
}

/// The sign-in answer: either a finished sign-in (`token` set) or an MFA challenge (`mfa` set).
#[derive(Clone, Deserialize, Default)]
#[serde(default)]
pub struct LoginResult {
    pub token: String,
    pub user_id: String,
    pub user: User,
    pub mfa: bool,
    pub ticket: String,
    pub totp: bool,
    pub webauthn: bool,
    pub backup_codes: bool,
}

impl fmt::Debug for LoginResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginResult")
            .field("token", &"[redacted]")
            .field("ticket", &"[redacted]")
            .field("mfa", &self.mfa)
            .field("totp", &self.totp)
            .field("backup_codes", &self.backup_codes)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct GatewayInfo {
    url: String,
}

/// A REST client for one instance and one credential.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    cred: Credential,
    endpoints: Endpoints,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("cred", &self.cred)
            .field("api", &self.endpoints.api)
            .finish_non_exhaustive()
    }
}

impl Client {
    pub fn new(http: reqwest::Client, endpoints: Endpoints, cred: Credential) -> Self {
        Self {
            http,
            cred,
            endpoints,
        }
    }

    pub fn credential(&self) -> &Credential {
        &self.cred
    }

    pub fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }

    /// The base for CDN paths (avatars, icons): the media host, or the API host when there is none.
    pub fn media_base(&self) -> &str {
        if self.endpoints.media.is_empty() {
            &self.endpoints.api
        } else {
            &self.endpoints.media
        }
    }

    /// The account the credential belongs to.
    pub async fn me(&self) -> Result<User> {
        let raw = self.send(Call::get("/users/@me".to_owned())).await?;
        decode(&raw)
    }

    /// The Gateway URL for a bot (`GET /gateway/bot`).
    pub async fn bot_gateway_url(&self) -> Result<String> {
        let raw = self.send(Call::get("/gateway/bot".to_owned())).await?;
        let info: GatewayInfo = decode(&raw)?;
        Ok(info.url)
    }

    /// Revoke the session behind a user token (`POST /auth/logout`).
    pub async fn logout(&self) -> Result<()> {
        self.send(Call::post("/auth/logout".to_owned())).await?;
        Ok(())
    }

    /// One page of messages, newest first.
    pub async fn messages(&self, channel: &str, query: MessageQuery<'_>) -> Result<Vec<Message>> {
        let mut call = Call::get(format!("/channels/{}/messages", seg(channel)))
            .query("limit", query.limit.to_string());
        if let Some(before) = query.before.filter(|b| !b.is_empty()) {
            call = call.query("before", before.to_owned());
        }
        let raw = self.send(call).await?;
        decode(&raw)
    }

    /// One message.
    pub async fn message(&self, channel: &str, message: &str) -> Result<Message> {
        let raw = self.send(Call::get(message_path(channel, message))).await?;
        decode(&raw)
    }

    /// Send a text message, or a message with files when `files` is not empty.
    pub async fn create_message(
        &self,
        channel: &str,
        body: &MessageBody,
        files: &[Upload],
    ) -> Result<Message> {
        let path = format!("/channels/{}/messages", seg(channel));
        let call = if files.is_empty() {
            Call::post(path).json(body)?
        } else {
            let mut body = body.clone();
            body.attachments = files
                .iter()
                .map(|f| AttachmentMeta {
                    id: f.id,
                    filename: f.filename.clone(),
                    content_type: f.content_type.clone(),
                })
                .collect();
            let multipart = multipart(&body, files)?;
            let mut call = Call::post(path);
            call.body = Some(multipart);
            call
        };
        let raw = self.send(call).await?;
        decode(&raw)
    }

    /// Replace the text of a message.
    pub async fn edit_message(&self, channel: &str, message: &str, content: &str) -> Result<()> {
        let call = Call::new(reqwest::Method::PATCH, message_path(channel, message))
            .json(&serde_json::json!({ "content": content }))?;
        self.send(call).await?;
        Ok(())
    }

    /// Delete a message.
    pub async fn delete_message(&self, channel: &str, message: &str) -> Result<()> {
        self.send(Call::new(
            reqwest::Method::DELETE,
            message_path(channel, message),
        ))
        .await?;
        Ok(())
    }

    /// Add (`add = true`) or remove the account's own reaction.
    pub async fn set_reaction(
        &self,
        channel: &str,
        message: &str,
        emoji: &str,
        add: bool,
    ) -> Result<()> {
        let method = if add {
            reqwest::Method::PUT
        } else {
            reqwest::Method::DELETE
        };
        let path = format!(
            "{}/reactions/{}/@me",
            message_path(channel, message),
            seg(emoji)
        );
        self.send(Call::new(method, path)).await?;
        Ok(())
    }

    /// Show the typing indicator in a channel.
    pub async fn typing(&self, channel: &str) -> Result<()> {
        self.send(Call::post(format!("/channels/{}/typing", seg(channel))))
            .await?;
        Ok(())
    }

    /// Mark a channel as read up to a message. Only user sessions may call it.
    pub async fn ack(&self, channel: &str, message: &str) -> Result<()> {
        let call = Call::post(format!("{}/ack", message_path(channel, message)))
            .json(&serde_json::json!({ "mention_count": 0 }))?;
        self.send(call).await?;
        Ok(())
    }

    /// Download media from the API host or the media host. Only https is accepted, and the
    /// credential is sent only to the API host.
    pub async fn download(&self, raw_url: &str) -> Result<Vec<u8>> {
        let url = Url::parse(raw_url)
            .map_err(|_| Error::Protocol("attachment URL is not valid".to_owned()))?;
        if url.scheme() != "https" {
            return Err(Error::Protocol("attachment URL is not https".to_owned()));
        }
        let key = host_key(&url)
            .ok_or_else(|| Error::Protocol("attachment URL has no host".to_owned()))?;
        let api = Url::parse(&self.endpoints.api)
            .ok()
            .and_then(|u| host_key(&u))
            .ok_or_else(|| Error::Protocol("the instance API URL is not valid".to_owned()))?;
        let media = if self.endpoints.media.is_empty() {
            api.clone()
        } else {
            Url::parse(&self.endpoints.media)
                .ok()
                .and_then(|u| host_key(&u))
                .ok_or_else(|| Error::Protocol("the instance media URL is not valid".to_owned()))?
        };
        if key != api && key != media {
            return Err(Error::Protocol(
                "attachment host is not the instance's host".to_owned(),
            ));
        }
        let mut request = self
            .http
            .get(url)
            .header(reqwest::header::USER_AGENT, USER_AGENT);
        if key == api && self.cred.has_token() {
            request = request.header(AUTHORIZATION, self.cred.authorization());
        }
        let mut response = request.send().await.map_err(network_error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(if status.is_server_error() {
                Error::Network(format!("media answered HTTP {status}"))
            } else {
                Error::Protocol(format!("media answered HTTP {status}"))
            });
        }
        read_limited(&mut response, MAX_MEDIA_BYTES).await
    }

    /// `POST /auth/login` with a password. A CAPTCHA challenge is solved and the call retried.
    pub async fn login(
        &self,
        login: &str,
        password: &str,
    ) -> std::result::Result<LoginResult, CallErr> {
        let body = serde_json::json!({ "login": login, "password": password });
        let body = &body;
        with_captcha(move |captcha| async move {
            let call = Call::post("/auth/login".to_owned())
                .json(body)?
                .anonymous()
                .captcha(captcha);
            let raw = self.send(call).await?;
            decode(&raw).map_err(CallErr::Other)
        })
        .await
    }

    /// `POST /auth/login/mfa/totp`: finish a password sign-in with an authenticator or backup code.
    pub async fn login_totp(
        &self,
        ticket: &str,
        code: &str,
    ) -> std::result::Result<LoginResult, CallErr> {
        let body = serde_json::json!({ "ticket": ticket, "code": code });
        let call = Call::post("/auth/login/mfa/totp".to_owned())
            .json(&body)?
            .anonymous();
        let raw = self.send(call).await?;
        decode(&raw).map_err(CallErr::Other)
    }

    /// Send a call, retrying a 429 as [`retry_delay`] allows.
    async fn send(&self, call: Call) -> std::result::Result<Vec<u8>, CallErr> {
        let mut attempt = 0;
        loop {
            match self.send_once(&call).await {
                Err(CallErr::Api(err)) => match retry_delay(&err, attempt, self.cred.is_bot()) {
                    Some(delay) => {
                        attempt += 1;
                        tokio::time::sleep(delay).await;
                    }
                    None => return Err(CallErr::Api(err)),
                },
                other => return other,
            }
        }
    }

    async fn send_once(&self, call: &Call) -> std::result::Result<Vec<u8>, CallErr> {
        let mut url = format!("{}/v1{}", self.endpoints.api, call.path);
        if !call.query.is_empty() {
            let pairs: Vec<String> = call
                .query
                .iter()
                .map(|(key, value)| format!("{key}={}", seg(value)))
                .collect();
            url.push('?');
            url.push_str(&pairs.join("&"));
        }
        let mut request = self
            .http
            .request(call.method.clone(), url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(ACCEPT, "application/json");
        if let Some(body) = &call.body {
            request = request
                .header(CONTENT_TYPE, body.content_type.as_str())
                .body(body.bytes.clone());
        }
        if !call.anonymous && self.cred.has_token() {
            request = request.header(AUTHORIZATION, self.cred.authorization());
        }
        if let Some(captcha) = &call.captcha {
            request = request.header("X-Captcha-Token", captcha.as_str());
        }
        let mut response = request
            .send()
            .await
            .map_err(|e| CallErr::Other(network_error(e)))?;
        let status = response.status();
        let retry_header = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok());
        let raw = read_limited(&mut response, MAX_JSON_BYTES)
            .await
            .map_err(CallErr::Other)?;
        if status.as_u16() >= 300 {
            return Err(CallErr::Api(ApiError::from_response(
                status.as_u16(),
                retry_header,
                &raw,
            )));
        }
        Ok(raw)
    }
}

/// Decode a JSON body. The serde error text describes positions only, never the content.
pub fn decode<T: DeserializeOwned>(raw: &[u8]) -> Result<T> {
    serde_json::from_slice(raw).map_err(|e| Error::Protocol(format!("Fluxer JSON: {e}")))
}

/// Percent-encode one path or query component.
fn seg(value: &str) -> String {
    utf8_percent_encode(value, SEGMENT).to_string()
}

fn message_path(channel: &str, message: &str) -> String {
    format!("/channels/{}/messages/{}", seg(channel), seg(message))
}

/// `(host, port)` of a URL, lower-cased, with the scheme's default port filled in.
fn host_key(url: &Url) -> Option<(String, u16)> {
    let host = url.host_str()?.to_ascii_lowercase();
    Some((host, url.port_or_known_default()?))
}

/// Build the multipart body of a message with files: `payload_json` first, then `files[id]`.
fn multipart(body: &MessageBody, files: &[Upload]) -> Result<Body> {
    let payload = serde_json::to_vec(body)
        .map_err(|e| Error::Internal(format!("encoding a message: {e}")))?;
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|e| Error::Internal(format!("random boundary: {e}")))?;
    let boundary = format!("chirper{}", hex::encode(random));
    let mut out =
        Vec::with_capacity(payload.len() + files.iter().map(|f| f.data.len()).sum::<usize>() + 512);
    out.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"payload_json\"\r\nContent-Type: application/json\r\n\r\n"
        )
        .as_bytes(),
    );
    out.extend_from_slice(&payload);
    out.extend_from_slice(b"\r\n");
    for file in files {
        let media = if file.content_type.is_empty() {
            "application/octet-stream"
        } else {
            file.content_type.as_str()
        };
        out.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"files[{}]\"; filename=\"{}\"\r\nContent-Type: {media}\r\n\r\n",
                file.id,
                quote_filename(&file.filename)
            )
            .as_bytes(),
        );
        out.extend_from_slice(&file.data);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Ok(Body {
        content_type: format!("multipart/form-data; boundary={boundary}"),
        bytes: out,
    })
}

/// Quote a file name for a `Content-Disposition` header: no line breaks, escaped quotes.
fn quote_filename(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '\r' && *c != '\n')
        .flat_map(|c| match c {
            '"' => vec!['\\', '"'],
            '\\' => vec!['\\', '\\'],
            other => vec![other],
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_debug_hides_the_token() {
        let cred = Credential::new("super-secret".to_owned(), true);
        let shown = format!("{cred:?}");
        assert!(!shown.contains("super-secret"), "leaked: {shown}");
        assert!(shown.contains("[redacted]"));
    }

    #[test]
    fn bot_credentials_use_the_bot_prefix() {
        assert_eq!(
            Credential::new("123.secret".to_owned(), true).authorization(),
            "Bot 123.secret"
        );
        assert_eq!(
            Credential::new("user-token".to_owned(), false).authorization(),
            "user-token"
        );
        assert!(!Credential::anonymous().has_token());
    }

    #[test]
    fn user_agent_names_the_client() {
        assert!(USER_AGENT.starts_with("Chirper (https://chirper.app, "));
        assert!(USER_AGENT.ends_with(')'));
    }

    #[test]
    fn global_429_for_a_user_token_is_not_retried() {
        let err = ApiError::from_response(
            429,
            None,
            br#"{"code":"RATE_LIMITED","message":"slow","global":true,"retry_after":0.001}"#,
        );
        assert_eq!(retry_delay(&err, 0, false), None);
    }

    #[test]
    fn route_429_is_retried_three_times_then_given_up() {
        let err = ApiError::from_response(
            429,
            None,
            br#"{"code":"RATE_LIMITED","message":"slow","global":false,"retry_after":0.001}"#,
        );
        assert_eq!(retry_delay(&err, 0, false), Some(Duration::from_secs(1)));
        assert_eq!(retry_delay(&err, 2, false), Some(Duration::from_secs(1)));
        assert_eq!(retry_delay(&err, 3, false), None);
    }

    #[test]
    fn global_429_is_retried_for_a_bot() {
        let err = ApiError::from_response(
            429,
            None,
            br#"{"code":"RATE_LIMITED","global":true,"retry_after":2.5}"#,
        );
        assert_eq!(
            retry_delay(&err, 0, true),
            Some(Duration::from_millis(2500))
        );
    }

    #[test]
    fn long_waits_are_not_retried() {
        let err = ApiError::from_response(
            429,
            None,
            br#"{"code":"RATE_LIMITED","global":false,"retry_after":61}"#,
        );
        assert_eq!(retry_delay(&err, 0, true), None);
    }

    #[test]
    fn retry_after_header_is_used_when_the_body_has_none() {
        let err = ApiError::from_response(429, Some(7), b"not json");
        assert_eq!(err.retry_after, Duration::from_secs(7));
        assert!(err.code.is_empty());
    }

    #[test]
    fn hostile_retry_after_values_do_not_panic() {
        let err = ApiError::from_response(429, None, br#"{"retry_after":-5}"#);
        assert_eq!(err.retry_after, Duration::ZERO);
        let err = ApiError::from_response(429, None, br#"{"retry_after":1e300}"#);
        assert_eq!(err.retry_after, Duration::from_secs(86_400));
    }

    #[test]
    fn status_maps_to_bridge_errors() {
        let map = |status: u16| Error::from(ApiError::from_response(status, None, b"{}"));
        assert!(matches!(map(401), Error::Auth(_)));
        assert!(matches!(map(403), Error::Protocol(_)));
        assert!(matches!(map(404), Error::Protocol(_)));
        assert!(matches!(map(503), Error::Network(_)));
        assert!(matches!(map(429), Error::RateLimited { retry_after_ms: 0 }));
    }

    #[test]
    fn captcha_error_is_parsed_with_its_challenge() {
        let body = br#"{"code":"CAPTCHA_REQUIRED","message":"x","captcha_provider":"altcha","altcha_challenge":{"parameters":{"algorithm":"PBKDF2/SHA-256"},"signature":"ab"}}"#;
        let err = ApiError::from_response(400, None, body);
        assert!(err.is_captcha_required());
        let challenge = err.captcha_challenge.unwrap_or_default();
        assert!(challenge.contains(r#""signature":"ab""#), "{challenge}");
    }

    #[test]
    fn captcha_from_another_provider_is_not_solved() {
        let body =
            br#"{"code":"CAPTCHA_REQUIRED","captcha_provider":"hcaptcha","altcha_challenge":{}}"#;
        assert!(!ApiError::from_response(400, None, body).is_captcha_required());
    }

    #[test]
    fn path_segments_are_escaped() {
        assert_eq!(seg("123"), "123");
        assert_eq!(seg("a b/c"), "a%20b%2Fc");
        assert_eq!(seg("👍"), "%F0%9F%91%8D");
        assert_eq!(seg("wave:123"), "wave%3A123");
        assert_eq!(message_path("10", "99"), "/channels/10/messages/99");
    }

    #[test]
    fn host_keys_compare_default_ports() {
        let a = Url::parse("https://Media.Example.test/x").expect("url");
        let b = Url::parse("https://media.example.test:443/y").expect("url");
        assert_eq!(host_key(&a), host_key(&b));
        let c = Url::parse("https://media.example.test:8443/").expect("url");
        assert_ne!(host_key(&a), host_key(&c));
    }

    #[test]
    fn multipart_has_payload_then_file_and_closes() {
        let body = MessageBody {
            content: Some("look".to_owned()),
            ..Default::default()
        };
        let files = vec![Upload {
            id: 0,
            filename: "a\"b.png".to_owned(),
            content_type: "image/png".to_owned(),
            data: b"PNGDATA".to_vec(),
        }];
        let mut with_files = body.clone();
        with_files.attachments = vec![AttachmentMeta {
            id: 0,
            filename: "a\"b.png".to_owned(),
            content_type: "image/png".to_owned(),
        }];
        let encoded = multipart(&with_files, &files).expect("multipart");
        let text = String::from_utf8_lossy(&encoded.bytes).to_string();
        let boundary = encoded
            .content_type
            .strip_prefix("multipart/form-data; boundary=")
            .expect("boundary");
        let payload_at = text.find("name=\"payload_json\"").expect("payload part");
        let file_at = text.find("name=\"files[0]\"").expect("file part");
        assert!(payload_at < file_at);
        assert!(text.contains(r#"filename="a\"b.png""#));
        assert!(text.contains("PNGDATA\r\n"));
        assert!(text.contains(r#""content":"look""#));
        assert!(text.ends_with(&format!("--{boundary}--\r\n")));
    }

    #[test]
    fn message_body_omits_empty_fields() {
        let body = MessageBody {
            content: Some("hi".to_owned()),
            ..Default::default()
        };
        let json = serde_json::to_string(&body).expect("json");
        assert_eq!(json, r#"{"content":"hi"}"#);
    }

    #[test]
    fn filenames_lose_line_breaks() {
        assert_eq!(quote_filename("a\r\nb\\c\"d"), "ab\\\\c\\\"d");
    }

    #[test]
    fn login_result_debug_hides_secrets() {
        let result = LoginResult {
            token: "tok-secret".to_owned(),
            ticket: "ticket-secret".to_owned(),
            ..Default::default()
        };
        let shown = format!("{result:?}");
        assert!(!shown.contains("tok-secret") && !shown.contains("ticket-secret"));
    }

    #[test]
    fn decode_errors_do_not_echo_the_body() {
        let err = decode::<crate::map::Message>(
            br#"{"id":"secret-id-value","content":{"secret-key":1}}"#,
        );
        match err {
            Err(Error::Protocol(text)) => assert!(!text.contains("secret-"), "leaked: {text}"),
            other => panic!("expected a protocol error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn transport_errors_drop_the_request_url() {
        // Port 9 on loopback is closed, so the connection is refused; the URL carries a signature.
        let http = reqwest::Client::new();
        let err = http
            .get("http://127.0.0.1:9/media.png?sig=SECRET-SIGNATURE")
            .send()
            .await
            .expect_err("nothing listens on port 9");
        let mapped = network_error(err);
        let text = format!("{mapped}");
        assert!(!text.contains("SECRET-SIGNATURE"), "leaked: {text}");
        assert!(matches!(mapped, Error::Network(_)));
    }
}
