//! The XRPC transport: one place for the request shapes, the auth headers, the response size cap
//! and the mapping of every HTTP status and XRPC error name onto the bridge's errors.

pub mod actor;
pub mod chat;
pub mod identity;
pub mod server;
pub mod types;

use reqwest::StatusCode;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use serde::de::DeserializeOwned;
use serde_json::Value;

use chirper_bridge::{Error, Result};

/// The largest response body the driver reads. A history page of 100 messages is far smaller.
pub const MAX_RESPONSE: usize = 8 * 1024 * 1024;
/// The wait given to a 429 that names none.
const DEFAULT_RETRY_MS: u64 = 30_000;
/// The chat service the PDS proxies `chat.bsky.*` calls to.
const CHAT_PROXY: &str = "did:web:api.bsky.chat#bsky_chat";
/// XRPC error names that mean the saved credentials are no longer accepted.
const AUTH_ERRORS: &[&str] = &[
    "InvalidToken",
    "AuthenticationRequired",
    "InvalidPassword",
    "AccountTakedown",
];

/// The failure of one XRPC call, before the account layer decides what it means.
#[derive(Debug)]
pub enum ApiError {
    /// The access token expired: refresh it and repeat the call.
    ExpiredToken,
    /// The account signs in with a second factor.
    AuthFactorRequired,
    /// Any other failure, already a bridge error.
    Other(Error),
}

impl ApiError {
    /// The bridge error this failure stands for. Expiry and factor requests are not errors on their
    /// own; reaching here means the caller had no way to deal with them.
    pub fn into_error(self) -> Error {
        match self {
            Self::ExpiredToken => Error::Auth("the Bluesky session expired".to_owned()),
            Self::AuthFactorRequired => Error::Auth("Bluesky asks for a second factor".to_owned()),
            Self::Other(err) => err,
        }
    }
}

impl From<Error> for ApiError {
    fn from(err: Error) -> Self {
        Self::Other(err)
    }
}

/// How one call authenticates: a bearer token, and whether the PDS proxies it to the chat service.
#[derive(Clone, Copy, Debug, Default)]
pub struct Auth<'a> {
    pub token: Option<&'a str>,
    pub chat: bool,
}

impl<'a> Auth<'a> {
    /// No credentials.
    pub const NONE: Self = Self {
        token: None,
        chat: false,
    };

    /// A bearer token for an `app.bsky` or `com.atproto` call.
    pub fn bearer(token: &'a str) -> Self {
        Self {
            token: Some(token),
            chat: false,
        }
    }

    /// A bearer token for a `chat.bsky` call, proxied to the chat service.
    pub fn chat(token: &'a str) -> Self {
        Self {
            token: Some(token),
            chat: true,
        }
    }
}

/// One PDS, reached over `https` (or `http` in tests).
#[derive(Clone)]
pub struct Pds {
    http: reqwest::Client,
    base: String,
}

impl Pds {
    pub fn new(http: reqwest::Client, base: impl Into<String>) -> Self {
        Self {
            http,
            base: base.into().trim_end_matches('/').to_owned(),
        }
    }

    /// A `GET` of an XRPC query.
    pub async fn query<T: DeserializeOwned>(
        &self,
        nsid: &str,
        params: &[(&str, String)],
        auth: Auth<'_>,
    ) -> std::result::Result<T, ApiError> {
        let request = self.http.get(self.url(nsid)).query(params);
        self.decode(request, auth).await
    }

    /// A `POST` of an XRPC procedure, with an optional JSON body.
    pub async fn procedure<T: DeserializeOwned>(
        &self,
        nsid: &str,
        body: Option<&Value>,
        auth: Auth<'_>,
    ) -> std::result::Result<T, ApiError> {
        let request = self.http.post(self.url(nsid));
        let request = match body {
            Some(body) => request.json(body),
            None => request,
        };
        self.decode(request, auth).await
    }

    fn url(&self, nsid: &str) -> String {
        format!("{}/xrpc/{nsid}", self.base)
    }

    async fn decode<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
        auth: Auth<'_>,
    ) -> std::result::Result<T, ApiError> {
        let body = execute(authorize(request, auth)).await?;
        // An empty answer is what a procedure without output sends; it reads as JSON `null`.
        let source: &[u8] = if body.is_empty() { b"null" } else { &body };
        serde_json::from_slice(source)
            .map_err(|err| ApiError::Other(Error::Protocol(format!("Bluesky JSON: {err}"))))
    }
}

fn authorize(request: reqwest::RequestBuilder, auth: Auth<'_>) -> reqwest::RequestBuilder {
    let request = match auth.token {
        Some(token) => request.bearer_auth(token),
        None => request,
    };
    if auth.chat {
        request.header("atproto-proxy", CHAT_PROXY)
    } else {
        request
    }
}

/// Send one request and return its body when the status is a success; otherwise the mapped failure.
async fn execute(request: reqwest::RequestBuilder) -> std::result::Result<Vec<u8>, ApiError> {
    let mut response = request.send().await.map_err(transport)?;
    let status = response.status();
    let headers = response.headers().clone();
    let body = read_capped(&mut response).await?;
    if status.is_success() {
        Ok(body)
    } else {
        Err(failure(status, &headers, &body))
    }
}

/// Read a response body, refusing one larger than [`MAX_RESPONSE`].
pub async fn read_capped(response: &mut reqwest::Response) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport)? {
        if body.len() + chunk.len() > MAX_RESPONSE {
            return Err(Error::Protocol(
                "Bluesky sent a response that is too large".to_owned(),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// A send or receive that failed below HTTP. The URL is dropped: it can carry a query string.
pub fn transport(err: reqwest::Error) -> Error {
    Error::Network(format!("Bluesky request failed: {}", err.without_url()))
}

/// The one status handler: what an XRPC failure means for the bridge.
pub fn failure(status: StatusCode, headers: &HeaderMap, body: &[u8]) -> ApiError {
    let name = error_name(body);
    match name.as_str() {
        "ExpiredToken" => return ApiError::ExpiredToken,
        "AuthFactorTokenRequired" => return ApiError::AuthFactorRequired,
        _ => {}
    }
    if AUTH_ERRORS.contains(&name.as_str()) {
        return ApiError::Other(Error::Auth(format!("Bluesky refused the session ({name})")));
    }
    let err = match status.as_u16() {
        401 | 403 => Error::Auth(format!("Bluesky refused the session ({status})")),
        429 => Error::RateLimited {
            retry_after_ms: retry_after_ms(headers),
        },
        500..=599 => Error::Network(format!("Bluesky answered {status}")),
        _ => Error::Protocol(format!("Bluesky answered {status} {name}")),
    };
    ApiError::Other(err)
}

/// The XRPC error name of a failure body, reduced to letters and digits. Messages are dropped on
/// purpose: they can echo request data.
fn error_name(body: &[u8]) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .as_ref()
        .and_then(|value| value.get("error"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(64)
        .collect()
}

fn retry_after_ms(headers: &HeaderMap) -> u64 {
    headers
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|text| text.trim().parse::<u64>().ok())
        .map_or(DEFAULT_RETRY_MS, |secs| secs.saturating_mul(1000))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn status_of(code: u16, body: &str) -> ApiError {
        failure(
            StatusCode::from_u16(code).expect("valid status"),
            &HeaderMap::new(),
            body.as_bytes(),
        )
    }

    fn is_auth(err: &ApiError) -> bool {
        matches!(err, ApiError::Other(Error::Auth(_)))
    }

    #[test]
    fn expired_tokens_and_factor_requests_are_named_not_mapped() {
        assert!(matches!(
            status_of(400, r#"{"error":"ExpiredToken"}"#),
            ApiError::ExpiredToken
        ));
        assert!(matches!(
            status_of(401, r#"{"error":"AuthFactorTokenRequired"}"#),
            ApiError::AuthFactorRequired
        ));
    }

    #[test]
    fn credential_refusals_are_auth_errors() {
        assert!(is_auth(&status_of(
            401,
            r#"{"error":"AuthenticationRequired"}"#
        )));
        assert!(is_auth(&status_of(400, r#"{"error":"InvalidToken"}"#)));
        assert!(is_auth(&status_of(403, "")));
    }

    #[test]
    fn server_errors_are_network_errors_and_rate_limits_keep_their_wait() {
        assert!(matches!(
            status_of(503, ""),
            ApiError::Other(Error::Network(_))
        ));
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("7"));
        match failure(StatusCode::TOO_MANY_REQUESTS, &headers, b"") {
            ApiError::Other(Error::RateLimited { retry_after_ms }) => {
                assert_eq!(retry_after_ms, 7000);
            }
            other => panic!("expected a rate limit, got {other:?}"),
        }
        match failure(StatusCode::TOO_MANY_REQUESTS, &HeaderMap::new(), b"") {
            ApiError::Other(Error::RateLimited { retry_after_ms }) => {
                assert_eq!(retry_after_ms, DEFAULT_RETRY_MS);
            }
            other => panic!("expected a rate limit, got {other:?}"),
        }
    }

    #[test]
    fn other_client_errors_are_protocol_errors_without_the_message() {
        let err = status_of(
            400,
            r#"{"error":"InvalidRequest","message":"token=secret123"}"#,
        );
        match err {
            ApiError::Other(Error::Protocol(text)) => {
                assert!(text.contains("InvalidRequest"));
                assert!(!text.contains("secret123"));
            }
            other => panic!("expected a protocol error, got {other:?}"),
        }
    }

    #[test]
    fn error_names_keep_letters_and_digits_only() {
        assert_eq!(error_name(br#"{"error":"Bad<Name>!"}"#), "BadName");
        assert_eq!(error_name(b"not json"), "");
        assert_eq!(error_name(br#"{"error":"x"}"#).len(), 1);
    }
}
