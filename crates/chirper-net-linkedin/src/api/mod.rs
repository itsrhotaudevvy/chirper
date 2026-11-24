//! The LinkedIn messaging client: one place for the browser identity, the session cookies, the
//! CSRF header and the mapping of every HTTP status onto the bridge's errors.
//!
//! The request shapes (paths, GraphQL query ids, headers, decoration ids) are the reference's
//! `linkedingo` package. Calls live in [`calls`], the outbound payloads in [`outbound`], the
//! realtime stream in [`realtime`] and the wire types in [`model`].

pub mod calls;
pub mod model;
pub mod outbound;
pub mod realtime;
pub mod urn;

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::{Method, RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;
use zeroize::Zeroize;

use chirper_bridge::{Error, Result};

/// The origin of every messaging request. Tests point the client at a local server instead.
pub const WWW: &str = "https://www.linkedin.com";
/// The user agent of the browser the reference impersonates, kept together with its client hints.
pub const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) \
    Chrome/141.0.0.0 Safari/537.36";
/// The `X-LI-Track` value the reference sends when none was captured.
pub const X_LI_TRACK: &str = r#"{"clientVersion":"1.13.40953","mpVersion":"1.13.40953","osName":"web","deviceFormFactor":"DESKTOP","mpName":"voyager-web","displayDensity":2,"displayWidth":2880,"displayHeight":1800}"#;
/// The `X-LI-Page-Instance` value the reference sends when none was captured.
pub const PAGE_INSTANCE: &str = "urn:li:page:messaging_thread;5accf988-7540-4d0a-8a28-a0732bf6de20";
/// The version of the web client the tracking headers claim.
pub const SERVICE_VERSION: &str = "1.13.40953";

/// The cookie that holds the session.
pub const COOKIE_SESSION: &str = "li_at";
/// The cookie whose value is also the CSRF token.
pub const COOKIE_CSRF: &str = "JSESSIONID";

/// The session cookies of one login. Values are wiped on drop and never printed.
#[derive(Clone, Default)]
pub struct Cookies {
    values: BTreeMap<String, String>,
}

impl Cookies {
    /// Cookies from name/value pairs. Surrounding quotes are removed, as browsers store them.
    pub fn new(values: BTreeMap<String, String>) -> Self {
        let values = values
            .into_iter()
            .map(|(name, value)| (name, value.trim().trim_matches('"').to_owned()))
            .filter(|(name, value)| !name.is_empty() && !value.is_empty())
            .collect();
        Self { values }
    }

    /// The value of one cookie.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    /// Whether the two cookies every request needs are present.
    pub fn is_complete(&self) -> bool {
        self.get(COOKIE_SESSION).is_some() && self.get(COOKIE_CSRF).is_some()
    }

    /// The `Cookie` header value, sorted by name.
    pub fn header(&self) -> String {
        self.values
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// The name/value map, for storing as the `cookies` secret.
    pub fn to_map(&self) -> BTreeMap<String, String> {
        self.values.clone()
    }
}

impl Drop for Cookies {
    fn drop(&mut self) {
        for value in self.values.values_mut() {
            value.zeroize();
        }
    }
}

impl fmt::Debug for Cookies {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

/// A messaging client for one login.
#[derive(Clone)]
pub struct Api {
    http: reqwest::Client,
    base: Arc<str>,
    cookies: Arc<Cookies>,
    user_agent: Arc<str>,
}

impl fmt::Debug for Api {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Api")
            .field("base", &self.base)
            .field("cookies", &self.cookies)
            .finish_non_exhaustive()
    }
}

/// A response whose body was read in full.
#[derive(Debug)]
pub struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Api {
    /// A client for `cookies`, talking to `base` (normally [`WWW`]).
    pub fn new(http: reqwest::Client, base: &str, cookies: Cookies, user_agent: &str) -> Self {
        Self {
            http,
            base: base.trim_end_matches('/').into(),
            cookies: Arc::new(cookies),
            user_agent: user_agent.into(),
        }
    }

    /// The CSRF token: the `JSESSIONID` value.
    pub fn csrf(&self) -> &str {
        self.cookies.get(COOKIE_CSRF).unwrap_or("")
    }

    /// A request to `path` on the base origin, with the browser identity and the session cookie.
    pub(crate) fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.request_url(method, &format!("{}{path}", self.base))
    }

    /// A request to an absolute URL. The session cookie only goes to the base origin and to
    /// LinkedIn's own hosts, never to a media or upload host.
    pub(crate) fn request_url(&self, method: Method, url: &str) -> RequestBuilder {
        let builder = browser_identity(self.http.request(method, url), &self.user_agent);
        if self.sends_cookies(url) {
            builder.header("Cookie", self.cookies.header())
        } else {
            builder
        }
    }

    /// Whether the session cookie may be sent to `url`.
    pub(crate) fn sends_cookies(&self, url: &str) -> bool {
        if url.starts_with(self.base.as_ref()) {
            return true;
        }
        reqwest::Url::parse(url)
            .ok()
            .and_then(|parsed| {
                parsed
                    .host_str()
                    .map(|host| host == "linkedin.com" || host.ends_with(".linkedin.com"))
            })
            .unwrap_or(false)
    }

    /// The headers every voyager request carries alongside the CSRF token.
    pub(crate) fn with_voyager_headers(&self, builder: RequestBuilder) -> RequestBuilder {
        builder
            .header("csrf-token", self.csrf())
            .header("Referer", format!("{}/messaging/", self.base))
            .header("X-LI-Page-Instance", PAGE_INSTANCE)
            .header("X-LI-Track", X_LI_TRACK)
            .header("X-RestLI-Protocol-Version", "2.0.0")
    }

    /// Send a request and read the answer. Transport failures become [`Error::Network`].
    pub(crate) async fn send(&self, builder: RequestBuilder) -> Result<Reply> {
        let response = builder.send().await.map_err(transport_error)?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.bytes().await.map_err(transport_error)?.to_vec();
        Ok(Reply {
            status,
            headers,
            body,
        })
    }

    /// Send a request, map a non-success status to an error, and return the body.
    pub(crate) async fn expect_ok(&self, builder: RequestBuilder) -> Result<Vec<u8>> {
        let reply = self.send(builder).await?;
        check(reply.status, &reply.headers)?;
        Ok(reply.body)
    }

    /// Send a request whose answer is a JSON document.
    pub(crate) async fn json<T: DeserializeOwned>(&self, builder: RequestBuilder) -> Result<T> {
        let body = self.expect_ok(builder).await?;
        serde_json::from_slice(&body).map_err(|e| Error::Protocol(format!("LinkedIn JSON: {e}")))
    }

    /// A JSON body with the given content type, the way the reference posts payloads.
    pub(crate) fn json_body<T: serde::Serialize + ?Sized>(
        builder: RequestBuilder,
        content_type: &str,
        payload: &T,
    ) -> Result<RequestBuilder> {
        let bytes = serde_json::to_vec(payload)
            .map_err(|e| Error::Internal(format!("encoding a LinkedIn payload: {e}")))?;
        Ok(builder.header("content-type", content_type).body(bytes))
    }
}

/// The identity headers of the browser the reference impersonates: the user agent and client hints.
pub(crate) fn browser_identity(builder: RequestBuilder, user_agent: &str) -> RequestBuilder {
    builder
        .header("User-Agent", user_agent)
        .header("Accept-Language", "en-US,en;q=0.9")
        .header(
            "Sec-Ch-Ua",
            r#""Chromium";v="141", "Google Chrome";v="141", "Not-A.Brand";v="99""#,
        )
        .header("Sec-Ch-Ua-Platform", r#""Linux""#)
        .header("Sec-Ch-Ua-Mobile", "?0")
        .header("Sec-Ch-Prefers-Color-Scheme", "light")
}

/// The content type of JSON payloads sent as plain text by the reference.
pub const CONTENT_PLAIN: &str = "text/plain;charset=UTF-8";
/// The content type of JSON payloads.
pub const CONTENT_JSON: &str = "application/json; charset=UTF-8";
/// The normalised JSON media type the voyager API answers in.
pub const CONTENT_NORMALIZED: &str = "application/vnd.linkedin.normalized+json+2.1";

/// Maps a transport failure. The URL is dropped, so no query value reaches an error message.
fn transport_error(error: reqwest::Error) -> Error {
    let reason = if error.is_timeout() {
        "the request timed out"
    } else if error.is_connect() {
        "could not connect"
    } else {
        "the request failed"
    };
    Error::Network(format!("LinkedIn: {reason}"))
}

/// Maps a status onto the driver's error model.
pub fn check(status: StatusCode, headers: &HeaderMap) -> Result<()> {
    if status.is_success() {
        return Ok(());
    }
    let code = status.as_u16();
    match code {
        300..=399 => Err(Error::Auth(format!(
            "LinkedIn redirected the request (HTTP {code}), the session may have ended"
        ))),
        401 | 403 => Err(Error::Auth(format!(
            "LinkedIn refused the session (HTTP {code})"
        ))),
        429 => Err(Error::RateLimited {
            retry_after_ms: retry_after_ms(headers),
        }),
        999 => Err(Error::Protocol(
            "LinkedIn blocked the request (HTTP 999)".to_owned(),
        )),
        500..=599 => Err(Error::Network(format!("LinkedIn returned HTTP {code}"))),
        _ => Err(Error::Protocol(format!(
            "LinkedIn rejected the request (HTTP {code})"
        ))),
    }
}

/// The wait a `Retry-After` header asks for, in milliseconds. One minute when it is absent.
pub fn retry_after_ms(headers: &HeaderMap) -> u64 {
    const DEFAULT_MS: u64 = 60_000;
    headers
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|text| text.trim().parse::<u64>().ok())
        .map(|seconds| seconds.saturating_mul(1000))
        .unwrap_or(DEFAULT_MS)
}

/// A random version-4 UUID in its canonical text form.
pub fn uuid_v4() -> String {
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

/// Characters `url.QueryEscape` leaves alone.
const QUERY: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// Escapes a value the way Go's `url.QueryEscape` does, which the reference applies to every
/// URN it places in a raw query string.
pub fn query_escape(value: &str) -> String {
    utf8_percent_encode(value, QUERY)
        .to_string()
        .replace("%20", "+")
}

/// Escapes a path segment the way the reference does for message URNs: colons stay as they are.
pub fn path_escape(value: &str) -> String {
    const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'~')
        .remove(b':');
    utf8_percent_encode(value, SEGMENT).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_escape_matches_go_for_urns() {
        assert_eq!(
            query_escape("urn:li:fsd_profile:ACoAA1"),
            "urn%3Ali%3Afsd_profile%3AACoAA1"
        );
        assert_eq!(query_escape("a b-c_d.e~f"), "a+b-c_d.e~f");
    }

    #[test]
    fn path_escape_keeps_colons_and_escapes_parentheses() {
        assert_eq!(
            path_escape("urn:li:msg_message:(urn:li:fsd_profile:A,2)"),
            "urn:li:msg_message:%28urn:li:fsd_profile:A%2C2%29"
        );
    }

    #[test]
    fn cookies_drop_quotes_and_empty_values() {
        let jar = Cookies::new(BTreeMap::from([
            ("JSESSIONID".to_owned(), "\"ajax:42\"".to_owned()),
            ("li_at".to_owned(), "token".to_owned()),
            ("empty".to_owned(), String::new()),
        ]));
        assert_eq!(jar.get("JSESSIONID"), Some("ajax:42"));
        assert_eq!(jar.header(), "JSESSIONID=ajax:42; li_at=token");
        assert!(jar.is_complete());
    }

    #[test]
    fn cookies_never_print_their_values() {
        let jar = Cookies::new(BTreeMap::from([("li_at".to_owned(), "secret".to_owned())]));
        assert_eq!(format!("{jar:?}"), "[redacted]");
    }

    #[test]
    fn statuses_map_onto_the_error_model() {
        let none = HeaderMap::new();
        assert!(check(StatusCode::OK, &none).is_ok());
        assert!(matches!(
            check(StatusCode::UNAUTHORIZED, &none),
            Err(Error::Auth(_))
        ));
        assert!(matches!(
            check(StatusCode::FORBIDDEN, &none),
            Err(Error::Auth(_))
        ));
        assert!(matches!(
            check(StatusCode::FOUND, &none),
            Err(Error::Auth(_))
        ));
        assert!(matches!(
            check(StatusCode::BAD_GATEWAY, &none),
            Err(Error::Network(_))
        ));
        assert!(matches!(
            check(StatusCode::NOT_FOUND, &none),
            Err(Error::Protocol(_))
        ));
        let blocked = check(StatusCode::from_u16(999).unwrap(), &none);
        assert!(matches!(blocked, Err(Error::Protocol(_))));
    }

    #[test]
    fn rate_limits_read_retry_after_in_seconds() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, "7".parse().unwrap());
        assert_eq!(retry_after_ms(&headers), 7000);
        assert_eq!(retry_after_ms(&HeaderMap::new()), 60_000);
        match check(StatusCode::TOO_MANY_REQUESTS, &headers) {
            Err(Error::RateLimited { retry_after_ms }) => assert_eq!(retry_after_ms, 7000),
            other => panic!("expected a rate limit, got {other:?}"),
        }
    }
}
