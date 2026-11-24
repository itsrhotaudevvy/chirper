//! The HTTP side of the Messages web protocol (`libgm/http.go`): where each endpoint lives, the
//! headers the web client sends, the cookies and SAPISID hash of a Google account, the retry of
//! server errors, and the mapping of HTTP statuses onto the bridge's errors.
//!
//! Requests are protobuf (`application/x-protobuf`) or pblite (`application/json+protobuf`); the
//! response is decoded by its own content type, as the Go client does.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use log::debug;
use prost::Message;
use reqwest::header::{CONTENT_TYPE, COOKIE, HeaderMap, HeaderValue, RETRY_AFTER, SET_COOKIE};
use reqwest::{Method, StatusCode};
use serde_json::Value;
use sha1::{Digest, Sha1};

use chirper_bridge::{Error, Result};

use crate::auth::AuthData;
use crate::proto::pairing::{AuthMessage, BrowserDetails, ConfigVersion, DEVICE_TYPE_TABLET, BROWSER_TYPE_OTHER};

/// Body type of pblite requests and responses.
pub const CONTENT_PBLITE: &str = "application/json+protobuf";
/// Body type of binary protobuf requests and responses.
pub const CONTENT_PROTOBUF: &str = "application/x-protobuf";

/// Attempts a non-streaming request gets when the server answers 5xx.
const SERVER_ERROR_ATTEMPTS: u32 = 3;
/// Base of the back-off between those attempts (attempt × this).
const SERVER_ERROR_RETRY: Duration = Duration::from_secs(1);
/// Lifetime of one long-poll request (the Go client uses 30 minutes).
pub const LONG_POLL_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// The web client's version, as the server checks it (`util.ConfigMessage`).
pub fn config_version() -> ConfigVersion {
    ConfigVersion {
        year: 2026,
        month: 9,
        day: 2,
        v1: 4,
        v2: 6,
    }
}

/// The browser the driver presents itself as (`util.BrowserDetailsMessage`).
pub fn browser_details() -> BrowserDetails {
    BrowserDetails {
        user_agent: USER_AGENT.to_owned(),
        browser_type: BROWSER_TYPE_OTHER,
        os: "libgm".to_owned(),
        device_type: DEVICE_TYPE_TABLET,
    }
}

const USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36";
const SEC_UA: &str = r#""Google Chrome";v="152", "Chromium";v="152", "Not-A.Brand";v="24""#;
const GOOGLE_API_KEY: &str = "AIzaSyCA4RsOZUFrm9whhtGosPlJLmVPnfSHKz8";
const MESSAGES_ORIGIN: &str = "https://messages.google.com";

/// Path of the pairing service.
pub const PAIRING: &str = "/$rpc/google.internal.communications.instantmessaging.v1.Pairing";
/// Path of the messaging service.
pub const MESSAGING: &str = "/$rpc/google.internal.communications.instantmessaging.v1.Messaging";
/// Path of the registration service.
pub const REGISTRATION: &str = "/$rpc/google.internal.communications.instantmessaging.v1.Registration";

/// The hosts the driver talks to. Production values are in [`Hosts::production`]; tests point all
/// of them at a local server with [`Hosts::under`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hosts {
    /// The web config (`…/web/config`).
    pub config: String,
    /// The instant messaging host (`https://instantmessaging-pa.googleapis.com`).
    pub instant: String,
    /// The Google account variant of the instant messaging host.
    pub instant_google: String,
    /// The media upload URL.
    pub upload: String,
}

impl Hosts {
    /// The public Google endpoints.
    pub fn production() -> Self {
        Self {
            config: "https://messages.google.com/web/config".to_owned(),
            instant: "https://instantmessaging-pa.googleapis.com".to_owned(),
            instant_google: "https://instantmessaging-pa.clients6.google.com".to_owned(),
            upload: "https://instantmessaging-pa.googleapis.com/upload".to_owned(),
        }
    }

    /// Every endpoint under one base URL, for tests against a local server.
    pub fn under(base: &str) -> Self {
        let base = base.trim_end_matches('/');
        Self {
            config: format!("{base}/web/config"),
            instant: base.to_owned(),
            instant_google: base.to_owned(),
            upload: format!("{base}/upload"),
        }
    }
}

/// A successful raw exchange.
#[derive(Debug)]
pub struct RawResponse {
    /// The response headers.
    pub headers: HeaderMap,
    /// The response body.
    pub body: Vec<u8>,
}

/// The result of one HTTP exchange, decoded by its content type.
#[derive(Debug)]
pub enum Body {
    /// `application/x-protobuf`.
    Binary(Vec<u8>),
    /// `application/json+protobuf` (or `text/plain` carrying pblite).
    PbLite(Value),
}

/// A client for one paired session. Cheap to clone; the auth state is shared.
#[derive(Clone)]
pub struct Api {
    http: reqwest::Client,
    long_poll: reqwest::Client,
    hosts: Hosts,
    auth: Arc<Mutex<AuthData>>,
}

impl std::fmt::Debug for Api {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Api")
            .field("hosts", &self.hosts)
            .finish_non_exhaustive()
    }
}

impl Api {
    /// A client over the shared HTTP client and the given auth state.
    pub fn new(http: reqwest::Client, hosts: Hosts, auth: Arc<Mutex<AuthData>>) -> Self {
        let long_poll = reqwest::Client::builder()
            .user_agent(concat!("Chirper/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .timeout(LONG_POLL_TIMEOUT)
            .build()
            .unwrap_or_else(|_| http.clone());
        Self {
            http,
            long_poll,
            hosts,
            auth,
        }
    }

    /// A snapshot of the auth state.
    pub fn auth(&self) -> AuthData {
        lock(&self.auth).clone()
    }

    /// Change the auth state under the lock.
    pub fn update_auth<T>(&self, f: impl FnOnce(&mut AuthData) -> T) -> T {
        f(&mut lock(&self.auth))
    }

    /// The `ReceiveMessages`, `SendMessage` and `AckMessages` URL for the session's kind of account.
    pub fn messaging_url(&self, method: &str) -> String {
        let host = if self.auth().is_google_account() {
            &self.hosts.instant_google
        } else {
            &self.hosts.instant
        };
        format!("{host}{MESSAGING}/{method}")
    }

    /// A pairing or registration endpoint on the instant messaging host.
    pub fn instant_url(&self, service: &str, method: &str) -> String {
        format!("{}{service}/{method}", self.hosts.instant)
    }

    /// The host of the Google account registration service (always the Google variant).
    pub fn google_registration_host(&self) -> &str {
        &self.hosts.instant_google
    }

    /// The media upload endpoint.
    pub fn upload_url(&self) -> &str {
        &self.hosts.upload
    }

    /// The `AuthMessage` every request carries.
    pub fn auth_message(&self, request_id: String) -> AuthMessage {
        let auth = self.auth();
        AuthMessage {
            request_id,
            network: auth.auth_network().to_owned(),
            tachyon_auth_token: auth.tachyon_token,
            config_version: Some(config_version()),
        }
    }

    /// Send a binary protobuf request and decode a binary response.
    pub async fn post_proto<Req: Message, Resp: Message + Default>(
        &self,
        url: &str,
        request: &Req,
    ) -> Result<Resp> {
        let body = request.encode_to_vec();
        match self.post(url, CONTENT_PROTOBUF, body).await? {
            Body::Binary(bytes) => Resp::decode(bytes.as_slice())
                .map_err(|_| Error::Protocol("a response is not valid protobuf".to_owned())),
            Body::PbLite(_) => Err(Error::Protocol(
                "the server answered pblite to a protobuf request".to_owned(),
            )),
        }
    }

    /// Send a pblite request and return the pblite response.
    pub async fn post_pblite(&self, url: &str, request: &Value) -> Result<Value> {
        let body = serde_json::to_vec(request)
            .map_err(|_| Error::Internal("a request could not be encoded".to_owned()))?;
        match self.post(url, CONTENT_PBLITE, body).await? {
            Body::PbLite(value) => Ok(value),
            Body::Binary(_) => Err(Error::Protocol(
                "the server answered protobuf to a pblite request".to_owned(),
            )),
        }
    }

    /// Send a request and return the body decoded by its content type.
    pub async fn post(&self, url: &str, content_type: &str, body: Vec<u8>) -> Result<Body> {
        let mut attempt = 1;
        loop {
            let response = self
                .relay_request(Method::POST, url, Some((content_type, body.clone())), &self.http)
                .send()
                .await
                .map_err(transport_error)?;
            self.store_cookies(response.headers());
            let status = response.status();
            if status.is_server_error() && attempt < SERVER_ERROR_ATTEMPTS {
                debug!("gmessages: HTTP {} from server, retrying", status.as_u16());
                tokio::time::sleep(SERVER_ERROR_RETRY * attempt).await;
                attempt += 1;
                continue;
            }
            return decode_response(response).await;
        }
    }

    /// Open a streaming long poll. The caller reads the body; it is not retried here.
    pub async fn open_stream(&self, url: &str, request: &Value) -> Result<reqwest::Response> {
        let body = serde_json::to_vec(request)
            .map_err(|_| Error::Internal("a request could not be encoded".to_owned()))?;
        let response = self
            .relay_request(
                Method::POST,
                url,
                Some((CONTENT_PBLITE, body)),
                &self.long_poll,
            )
            .send()
            .await
            .map_err(transport_error)?;
        self.store_cookies(response.headers());
        check_status(&response)?;
        Ok(response)
    }

    /// `GET` the web config and decode it.
    pub async fn get_config(&self) -> Result<Body> {
        let mut request = self.http.get(&self.hosts.config);
        request = request.headers(self.headers(None, "*/*", true));
        let response = request.send().await.map_err(transport_error)?;
        self.store_cookies(response.headers());
        decode_response(response).await
    }

    /// One raw exchange with the relay headers, cookies and the given extra headers. Used by the
    /// media upload and download, whose headers are their own. Non-2xx answers become errors.
    pub async fn raw(
        &self,
        method: Method,
        url: &str,
        extra: &[(&'static str, String)],
        body: Option<Vec<u8>>,
    ) -> Result<RawResponse> {
        let mut headers = self.headers(None, "*/*", false);
        for (name, value) in extra {
            if let Ok(value) = HeaderValue::from_str(value) {
                headers.insert(*name, value);
            }
        }
        let mut request = self.http.request(method, url).headers(headers);
        if let Some(body) = body {
            request = request.body(body);
        }
        let response = request.send().await.map_err(transport_error)?;
        self.store_cookies(response.headers());
        let status = response.status();
        let response_headers = response.headers().clone();
        if !status.is_success() {
            return Err(status_error(status, &response_headers));
        }
        let bytes = response.bytes().await.map_err(transport_error)?;
        Ok(RawResponse {
            headers: response_headers,
            body: bytes.to_vec(),
        })
    }

    /// Fetch a media blob or any other GET the driver makes against the instant messaging host.
    pub async fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let response = self
            .relay_request(Method::GET, url, None, &self.http)
            .send()
            .await
            .map_err(transport_error)?;
        self.store_cookies(response.headers());
        check_status(&response)?;
        let bytes = response.bytes().await.map_err(transport_error)?;
        Ok(bytes.to_vec())
    }

    fn relay_request(
        &self,
        method: Method,
        url: &str,
        body: Option<(&str, Vec<u8>)>,
        client: &reqwest::Client,
    ) -> reqwest::RequestBuilder {
        let (content_type, bytes) = match body {
            Some((content_type, bytes)) => (Some(content_type), Some(bytes)),
            None => (None, None),
        };
        let mut request = client
            .request(method, url)
            .headers(self.headers(content_type, "*/*", false));
        if let Some(bytes) = bytes {
            request = request.body(bytes);
        }
        request
    }

    /// The headers of `util.BuildRelayHeaders`, plus the cookies and SAPISID hash of an account.
    fn headers(&self, content_type: Option<&str>, accept: &str, config_get: bool) -> HeaderMap {
        let mut headers = HeaderMap::new();
        let mut set = |name: &'static str, value: &str| {
            if let Ok(value) = HeaderValue::from_str(value) {
                headers.insert(name, value);
            }
        };
        set("sec-ch-ua", SEC_UA);
        if !config_get {
            set("x-user-agent", "grpc-web-javascript/0.1");
            set("x-goog-api-key", GOOGLE_API_KEY);
        }
        if let Some(content_type) = content_type {
            set("content-type", content_type);
        }
        set("sec-ch-ua-mobile", "?1");
        set("user-agent", USER_AGENT);
        set("sec-ch-ua-platform", "\"Android\"");
        set("accept", accept);
        if config_get {
            set("sec-fetch-site", "same-origin");
        } else {
            set("origin", MESSAGES_ORIGIN);
            set("sec-fetch-site", "cross-site");
        }
        set("sec-fetch-mode", "cors");
        set("sec-fetch-dest", "empty");
        set("referer", "https://messages.google.com/");
        set("accept-language", "en-US,en;q=0.9");
        let auth = self.auth();
        if !auth.cookies.is_empty() {
            let cookie = auth
                .cookies
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join("; ");
            set("cookie", &cookie);
            if let Some(sapisid) = auth.cookies.get("SAPISID") {
                set("authorization", &sapisid_hash(MESSAGES_ORIGIN, sapisid, unix_seconds()));
            }
        }
        headers
    }

    fn store_cookies(&self, headers: &HeaderMap) {
        let fresh: Vec<(String, String)> = headers
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .filter_map(|line| {
                let pair = line.split(';').next()?;
                let (name, value) = pair.split_once('=')?;
                Some((name.trim().to_owned(), value.trim().to_owned()))
            })
            .collect();
        if fresh.is_empty() {
            return;
        }
        self.update_auth(|auth| {
            if auth.cookies.is_empty() {
                return;
            }
            for (name, value) in fresh {
                auth.cookies.insert(name, value);
            }
        });
    }
}

/// `SAPISIDHASH <ts>_<sha1(ts sapisid origin)>`, the `Authorization` header of a Google account.
pub fn sapisid_hash(origin: &str, sapisid: &str, timestamp: u64) -> String {
    let digest = Sha1::digest(format!("{timestamp} {sapisid} {origin}").as_bytes());
    format!("SAPISIDHASH {timestamp}_{}", hex::encode(digest))
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn lock(auth: &Mutex<AuthData>) -> MutexGuard<'_, AuthData> {
    match auth.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn transport_error(err: reqwest::Error) -> Error {
    // The URL can carry nothing secret, but the message of a reqwest error is summarised anyway.
    if err.is_timeout() {
        Error::Network("the Messages request timed out".to_owned())
    } else {
        Error::Network("the Messages request could not be completed".to_owned())
    }
}

/// Map a non-success status onto the bridge's errors (401/403 → auth, 429 → rate limit, 5xx →
/// network, other → protocol). Response bodies are never copied into the error.
pub fn check_status(response: &reqwest::Response) -> Result<()> {
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    Err(status_error(status, response.headers()))
}

fn status_error(status: StatusCode, headers: &HeaderMap) -> Error {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Error::Auth(format!(
            "Google Messages refused the session (HTTP {})",
            status.as_u16()
        )),
        StatusCode::TOO_MANY_REQUESTS => Error::RateLimited {
            retry_after_ms: retry_after_ms(headers),
        },
        s if s.is_server_error() => Error::Network(format!(
            "Google Messages failed (HTTP {})",
            s.as_u16()
        )),
        s => Error::Protocol(format!("Google Messages answered HTTP {}", s.as_u16())),
    }
}

/// `Retry-After` in seconds, as milliseconds; 30 seconds when absent or unreadable.
fn retry_after_ms(headers: &HeaderMap) -> u64 {
    headers
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|text| text.trim().parse::<u64>().ok())
        .map(|seconds| seconds.saturating_mul(1000))
        .unwrap_or(30_000)
}

async fn decode_response(response: reqwest::Response) -> Result<Body> {
    let status = response.status();
    let headers = response.headers().clone();
    if !status.is_success() {
        return Err(status_error(status, &headers));
    }
    let bytes = response.bytes().await.map_err(transport_error)?;
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match content_type.as_str() {
        CONTENT_PROTOBUF => Ok(Body::Binary(bytes.to_vec())),
        CONTENT_PBLITE | "text/plain" => {
            let value = serde_json::from_slice(&bytes)
                .map_err(|_| Error::Protocol("a pblite response is not JSON".to_owned()))?;
            Ok(Body::PbLite(value))
        }
        _ => Err(Error::Protocol(format!(
            "unexpected response content type {content_type:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sapisid_hash_matches_the_google_scheme() {
        // sha1("1700000000 abc https://messages.google.com")
        let hash = sapisid_hash(MESSAGES_ORIGIN, "abc", 1_700_000_000);
        let expected = hex::encode(Sha1::digest(
            b"1700000000 abc https://messages.google.com",
        ));
        assert_eq!(hash, format!("SAPISIDHASH 1700000000_{expected}"));
    }

    #[test]
    fn statuses_map_onto_the_bridge_errors() {
        let none = HeaderMap::new();
        assert!(matches!(
            status_error(StatusCode::UNAUTHORIZED, &none),
            Error::Auth(_)
        ));
        assert!(matches!(
            status_error(StatusCode::FORBIDDEN, &none),
            Error::Auth(_)
        ));
        assert!(matches!(
            status_error(StatusCode::BAD_GATEWAY, &none),
            Error::Network(_)
        ));
        assert!(matches!(
            status_error(StatusCode::BAD_REQUEST, &none),
            Error::Protocol(_)
        ));
        match status_error(StatusCode::TOO_MANY_REQUESTS, &none) {
            Error::RateLimited { retry_after_ms } => assert_eq!(retry_after_ms, 30_000),
            other => panic!("expected a rate limit, got {other:?}"),
        }
    }

    #[test]
    fn retry_after_seconds_become_milliseconds() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("7"));
        assert_eq!(retry_after_ms(&headers), 7_000);
    }

    #[test]
    fn the_test_hosts_share_one_base() {
        let hosts = Hosts::under("http://127.0.0.1:9/");
        assert_eq!(hosts.config, "http://127.0.0.1:9/web/config");
        assert_eq!(hosts.upload, "http://127.0.0.1:9/upload");
        assert_eq!(hosts.instant, "http://127.0.0.1:9");
    }
}
