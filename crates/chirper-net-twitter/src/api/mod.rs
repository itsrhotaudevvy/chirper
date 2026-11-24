//! The X web client: one place for the bearer token, the browser fingerprint, the CSRF and cookie
//! headers, and the mapping of every answer onto the bridge's errors.
//!
//! Reference: `twittermeow/http.go`, `headers.go`, `client.go` (`makeAPIRequest`).

pub mod cookies;
pub mod dm;
pub mod media;
pub mod model;
pub mod query;
pub mod transaction;

use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::Value;

use chirper_bridge::{Error, Result};

use cookies::Cookies;

/// The web origin of the account pages and the DM endpoints.
pub const WEB: &str = "https://x.com";
/// The API origin of the GraphQL mutations.
pub const API: &str = "https://api.x.com";
/// The origin of the legacy media upload.
pub const UPLOAD: &str = "https://upload.x.com";
/// The origin of the scripts the web client loads.
pub const SCRIPTS: &str = "https://abs.twimg.com";
/// The public bearer token every web client sends.
pub const BEARER: &str = "Bearer AAAAAAAAAAAAAAAAAAAAANRILgAAAAAAnNwIzUejRCOuH5E6I8xnZz4puTs%3D1Zv7ttfk8LF81IUq16cHjhLTvJu4FA33AGWWjCpTnA";
/// The browser every request pretends to be until the login captured the real one.
pub const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
/// The client-hint header of the default browser.
const SEC_CH_UA: &str =
    "\"Chromium\";v=\"141\", \"Google Chrome\";v=\"141\", \"Not-A.Brand\";v=\"99\"";
/// Largest answer body the client reads.
const MAX_BODY: usize = 64 * 1024 * 1024;
/// Error codes X uses for a session that is no longer valid.
const AUTH_CODES: [i64; 5] = [32, 64, 89, 215, 326];
/// Error code X uses for a rate limit.
const RATE_LIMIT_CODE: i64 = 88;

/// Where each origin lives. Tests point all three at one local server.
#[derive(Clone, Debug)]
pub struct Bases {
    pub web: String,
    pub api: String,
    pub upload: String,
    pub scripts: String,
}

impl Default for Bases {
    fn default() -> Self {
        Self {
            web: WEB.to_owned(),
            api: API.to_owned(),
            upload: UPLOAD.to_owned(),
            scripts: SCRIPTS.to_owned(),
        }
    }
}

/// The body of one request.
pub enum Body {
    None,
    Json(Value),
    Form(String),
    /// Raw bytes with their media type, such as a multipart segment.
    Raw {
        content_type: String,
        bytes: Vec<u8>,
    },
}

/// The answer of one request, before it is judged.
pub struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

/// An authenticated client for one login.
#[derive(Clone)]
pub struct Api {
    http: reqwest::Client,
    bases: Bases,
    cookies: Cookies,
    client_uuid: String,
    user_agent: String,
    /// The signer of `x-client-transaction-id`, when the messages page could be read.
    signer: Option<transaction::Signer>,
}

impl std::fmt::Debug for Api {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Api")
            .field("bases", &self.bases)
            .field("cookies", &self.cookies)
            .finish_non_exhaustive()
    }
}

/// Per-request choices that vary between endpoints.
#[derive(Default, Clone)]
pub struct Opts<'a> {
    /// The `Referer` of the request, when the page it belongs to is known.
    pub referer: Option<&'a str>,
    /// Send `x-client-uuid`, which the DM endpoints check.
    pub client_uuid: bool,
}

impl Api {
    /// A client for one login's cookies.
    pub fn new(http: reqwest::Client, cookies: Cookies, bases: Bases) -> Self {
        Self {
            http,
            bases,
            cookies,
            client_uuid: random_uuid(),
            user_agent: USER_AGENT.to_owned(),
            signer: None,
        }
    }

    /// Use the browser fingerprint a login captured.
    pub fn with_user_agent(mut self, user_agent: &str) -> Self {
        if !user_agent.trim().is_empty() {
            self.user_agent = user_agent.trim().to_owned();
        }
        self
    }

    /// The origins this client talks to.
    pub fn bases(&self) -> &Bases {
        &self.bases
    }

    /// Use the transaction-id signer of this login's messages page.
    pub fn with_signer(mut self, signer: Option<transaction::Signer>) -> Self {
        self.signer = signer;
        self
    }

    /// Headers every API request carries: bearer, cookies, CSRF and the browser fingerprint.
    fn headers(&self, method: &Method, url: &str, opts: &Opts<'_>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        let mut put = |name: &'static str, value: &str| {
            if let Ok(value) = HeaderValue::from_str(value) {
                headers.insert(HeaderName::from_static(name), value);
            }
        };
        put("user-agent", &self.user_agent);
        put("sec-ch-ua", SEC_CH_UA);
        put("sec-ch-ua-platform", "\"Linux\"");
        put("sec-ch-ua-mobile", "?0");
        put("accept-language", "en");
        put("accept", "*/*");
        put("sec-fetch-dest", "empty");
        put("sec-fetch-mode", "cors");
        put("sec-fetch-site", "same-origin");
        put("origin", WEB);
        put("referer", opts.referer.unwrap_or("https://x.com/"));
        put("authorization", BEARER);
        put("cookie", &self.cookies.header());
        put("x-csrf-token", self.cookies.get(cookies::CT0));
        put("x-twitter-auth-type", "OAuth2Session");
        put("x-twitter-active-user", "yes");
        put("x-twitter-client-language", "en");
        let path =
            reqwest::Url::parse(url).map_or_else(|_| "/".to_owned(), |u| u.path().to_owned());
        let transaction = self.signer.as_ref().map_or_else(
            || transaction::FALLBACK_ID.to_owned(),
            |s| s.header(method.as_str(), &path),
        );
        put("x-client-transaction-id", &transaction);
        if opts.client_uuid {
            put("x-client-uuid", &self.client_uuid);
        }
        headers
    }

    /// Send one request and return the raw answer. Transport failures are `Network` errors.
    pub async fn send(
        &self,
        method: Method,
        url: &str,
        body: Body,
        opts: &Opts<'_>,
    ) -> Result<Reply> {
        let mut headers = self.headers(&method, url, opts);
        let mut request = self.http.request(method.clone(), url);
        match body {
            Body::None => {}
            Body::Json(value) => {
                put_content_type(&mut headers, "application/json");
                request = request.body(value.to_string());
            }
            Body::Form(text) => {
                put_content_type(&mut headers, "application/x-www-form-urlencoded");
                request = request.body(text);
            }
            Body::Raw {
                content_type,
                bytes,
            } => {
                put_content_type(&mut headers, &content_type);
                request = request.body(bytes);
            }
        }
        let response = request
            .headers(headers)
            .send()
            .await
            .map_err(|err| Error::Network(err.to_string()))?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = read_capped(response).await?;
        Ok(Reply {
            status,
            headers,
            body,
        })
    }

    /// A request whose answer must be a success; the body is returned.
    pub async fn call(
        &self,
        method: Method,
        url: &str,
        body: Body,
        opts: &Opts<'_>,
    ) -> Result<Vec<u8>> {
        let reply = self.send(method, url, body, opts).await?;
        check(reply.status, &reply.headers, &reply.body)?;
        Ok(reply.body)
    }

    /// Download a media file. A file larger than `cap` gives `None`. Only the web origins and the
    /// public CDN are fetched; the public CDN gets no session cookies.
    pub async fn download(&self, url: &str, cap: u64) -> Result<Option<Vec<u8>>> {
        let parsed = reqwest::Url::parse(url)
            .map_err(|_| Error::Protocol("X sent a media URL that is not one".to_owned()))?;
        let own = [
            &self.bases.web,
            &self.bases.api,
            &self.bases.upload,
            &self.bases.scripts,
        ]
        .iter()
        .any(|base| url.starts_with(base.as_str()));
        let cdn = parsed.scheme() == "https"
            && parsed.host_str().is_some_and(|h| h.ends_with("twimg.com"));
        if !own && !cdn {
            return Err(Error::Protocol("refusing a media URL outside X".to_owned()));
        }
        let mut request = self.http.get(url).header("user-agent", &self.user_agent);
        if !cdn {
            request = request.headers(self.headers(&Method::GET, url, &Opts::default()));
        }
        let mut response = request
            .send()
            .await
            .map_err(|err| Error::Network(err.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Network(format!("media answered {status}")));
        }
        if response.content_length().is_some_and(|len| len > cap) {
            return Ok(None);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|err| Error::Network(err.to_string()))?
        {
            if body.len() as u64 + chunk.len() as u64 > cap {
                return Ok(None);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Some(body))
    }

    /// A request whose answer is a JSON document.
    pub async fn call_json<T: DeserializeOwned>(
        &self,
        method: Method,
        url: &str,
        body: Body,
        opts: &Opts<'_>,
    ) -> Result<T> {
        let bytes = self.call(method, url, body, opts).await?;
        parse(&bytes)
    }
}

/// Parse a JSON answer. Anything unexpected is a protocol error, never a panic.
pub fn parse<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|err| Error::Protocol(format!("X JSON: {err}")))
}

/// Judge an answer: success passes, and every failure is mapped onto a bridge error without its
/// body (which may echo session data).
pub fn check(status: StatusCode, headers: &HeaderMap, body: &[u8]) -> Result<()> {
    if status.is_success() {
        return Ok(());
    }
    let code = error_code(body);
    if status == StatusCode::TOO_MANY_REQUESTS || code == Some(RATE_LIMIT_CODE) {
        return Err(Error::RateLimited {
            retry_after_ms: retry_after_ms(headers),
        });
    }
    if status == StatusCode::UNAUTHORIZED
        || status == StatusCode::FORBIDDEN
        || code.is_some_and(|c| AUTH_CODES.contains(&c))
    {
        return Err(Error::Auth("X refused the session".to_owned()));
    }
    if status.is_server_error() {
        return Err(Error::Network(format!("X answered {status}")));
    }
    Err(Error::Protocol(format!(
        "X answered {status}{}",
        code.map(|c| format!(" (code {c})")).unwrap_or_default()
    )))
}

/// The first error code of an X error document, when there is one.
pub fn error_code(body: &[u8]) -> Option<i64> {
    let value: Value = serde_json::from_slice(body).ok()?;
    value
        .get("errors")?
        .as_array()?
        .first()?
        .get("code")?
        .as_i64()
}

/// The wait a rate-limit answer asks for, in milliseconds. X sends the reset as Unix seconds.
fn retry_after_ms(headers: &HeaderMap) -> u64 {
    const DEFAULT_MS: u64 = 60_000;
    let reset = headers
        .get("x-rate-limit-reset")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    let Some(reset) = reset else {
        return DEFAULT_MS;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    reset
        .saturating_sub(now)
        .saturating_mul(1000)
        .clamp(1000, 3_600_000)
}

fn put_content_type(headers: &mut HeaderMap, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        headers.insert(CONTENT_TYPE, value);
    }
}

/// Read a body, refusing anything larger than the client's cap.
pub async fn read_capped(mut response: reqwest::Response) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|err| Error::Network(err.to_string()))?
    {
        if body.len() + chunk.len() > MAX_BODY {
            return Err(Error::Protocol(
                "X sent an answer that is too large".to_owned(),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// A random version-4 UUID, the form `x-client-uuid` carries.
pub fn random_uuid() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unauthorised_and_auth_codes_are_auth_errors() {
        let none = HeaderMap::new();
        assert!(matches!(
            check(StatusCode::UNAUTHORIZED, &none, b""),
            Err(Error::Auth(_))
        ));
        assert!(matches!(
            check(
                StatusCode::BAD_REQUEST,
                &none,
                br#"{"errors":[{"code":89}]}"#
            ),
            Err(Error::Auth(_))
        ));
    }

    #[test]
    fn rate_limits_are_rate_limited_errors() {
        let none = HeaderMap::new();
        assert!(matches!(
            check(StatusCode::TOO_MANY_REQUESTS, &none, b""),
            Err(Error::RateLimited {
                retry_after_ms: 60_000
            })
        ));
        assert!(matches!(
            check(
                StatusCode::BAD_REQUEST,
                &none,
                br#"{"errors":[{"code":88}]}"#
            ),
            Err(Error::RateLimited { .. })
        ));
    }

    #[test]
    fn server_errors_are_network_errors_and_others_protocol_errors() {
        let none = HeaderMap::new();
        assert!(matches!(
            check(StatusCode::BAD_GATEWAY, &none, b""),
            Err(Error::Network(_))
        ));
        let other = check(StatusCode::NOT_FOUND, &none, b"secret body").unwrap_err();
        assert!(matches!(other, Error::Protocol(ref m) if !m.contains("secret")));
    }

    #[test]
    fn a_success_passes() {
        assert!(check(StatusCode::OK, &HeaderMap::new(), b"").is_ok());
    }

    #[test]
    fn random_uuids_have_the_version_four_shape() {
        let id = random_uuid();
        assert_eq!(id.len(), 36);
        assert_eq!(id.as_bytes()[14], b'4');
        assert!(matches!(id.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
        assert_ne!(id, random_uuid());
    }

    #[test]
    fn parse_turns_garbage_into_a_protocol_error() {
        let parsed: Result<Value> = parse(b"{not json");
        assert!(matches!(parsed, Err(Error::Protocol(_))));
    }
}
