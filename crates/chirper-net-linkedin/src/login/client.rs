//! The HTTP side of the password login: one cookie jar per attempt (shared with the checkpoint
//! that may follow it), redirects followed by hand so that every hop is checked for origin and
//! path, the status classes of the reference, and the posts of the server request.
//!
//! Reference: `linkedingo/login.go` (`PasswordLoginClient`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rand::RngCore;
use reqwest::header::{COOKIE, HeaderMap, SET_COOKIE};
use reqwest::{Method, Url};
use serde_json::{Value, json};

use super::checkpoint::Checkpoint;
use super::sdui::{self, AUTH_REQUEST, CHALLENGE_ROUTE_REQUEST, Page};
use crate::api::{self, USER_AGENT};

/// The page the password form lives on.
pub(super) const LOGIN_PATH: &str = "/login";
/// How many redirects a bootstrap or checkpoint load follows.
pub(super) const MAX_REDIRECTS: usize = 5;
/// The largest answer body the login reads.
const MAX_BODY: usize = 8 << 20;
/// The `JSESSIONID` cookie: the CSRF token.
const COOKIE_CSRF: &str = "JSESSIONID";
/// The `li_at` cookie: the session.
const COOKIE_SESSION: &str = "li_at";

/// Why a login step failed, as the reference classifies it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// The service could not be reached, or answered 5xx.
    Unavailable,
    /// HTTP 429.
    RateLimited,
    /// HTTP 403 or 999: the client looks automated.
    Blocked,
    /// The page or the answer is in a shape the driver does not know.
    Unsupported,
    /// HTTP 4xx, or an answer that names a credential or code error.
    Rejected,
    /// The server wants more verification.
    Challenge,
    /// The answer could not be read.
    BadResponse,
    /// The account is in a region with its own site.
    RegionLocked,
}

/// What a failed step leaves for the step that follows it.
pub(super) enum Attached {
    /// Nothing: the failure does not concern the pending checkpoint.
    Nothing,
    /// The pending checkpoint, which the failure came from.
    Current,
    /// A checkpoint the failure carries, to become the pending one.
    New(Box<Checkpoint>),
}

/// A failed login step.
pub(super) struct Failure {
    pub(super) kind: Kind,
    /// Where it failed, as the reference names the stages.
    pub(super) stage: &'static str,
    pub(super) status: u16,
    /// The wait a rate limit asks for, in milliseconds.
    pub(super) retry_after_ms: u64,
    pub(super) attached: Attached,
}

impl Failure {
    /// A failure with no status and nothing attached.
    pub(super) fn new(kind: Kind, stage: &'static str, status: u16) -> Self {
        Self {
            kind,
            stage,
            status,
            retry_after_ms: 0,
            attached: Attached::Nothing,
        }
    }
}

/// One answer from the server.
pub(super) struct Reply {
    pub(super) status: u16,
    pub(super) headers: HeaderMap,
    pub(super) body: Vec<u8>,
}

impl Reply {
    /// The redirect target, as the server wrote it.
    pub(super) fn location(&self) -> Option<&str> {
        header_str(&self.headers, "location")
    }

    /// Whether the status is a redirect.
    pub(super) fn is_redirect(&self) -> bool {
        (300..400).contains(&self.status)
    }
}

/// The session the login ends with: the cookies, as the server set them.
pub(super) struct Authenticated {
    pub(super) cookies: BTreeMap<String, String>,
}

/// The answer of one server request: a session, or an inline challenge route to post back.
pub(super) enum Post {
    Done(Authenticated),
    Route(Value),
}

/// The client of one login attempt. Clones share the jar.
#[derive(Clone)]
pub(super) struct Client {
    http: reqwest::Client,
    /// The origin, without a trailing slash, e.g. `https://www.linkedin.com`.
    base: Arc<str>,
    jar: Arc<Mutex<BTreeMap<String, String>>>,
}

impl Client {
    /// A client for `base`, with an empty jar.
    pub(super) fn new(http: reqwest::Client, base: &str) -> Self {
        Self {
            http,
            base: base.trim_end_matches('/').into(),
            jar: Arc::default(),
        }
    }

    /// The origin this client talks to.
    pub(super) fn base(&self) -> &str {
        &self.base
    }

    /// Whether the jar holds the session and CSRF cookies.
    pub(super) fn has_session(&self) -> bool {
        self.cookie(COOKIE_SESSION).is_some() && self.csrf().is_some()
    }

    /// The CSRF token: the `JSESSIONID` value without its quotes.
    pub(super) fn csrf(&self) -> Option<String> {
        self.cookie(COOKIE_CSRF)
            .map(|value| value.trim_matches('"').to_owned())
            .filter(|value| !value.is_empty())
    }

    /// The session as the jar holds it.
    pub(super) fn authenticated(&self) -> Authenticated {
        Authenticated {
            cookies: self.jar_guard().clone(),
        }
    }

    /// Resolves a location against `current`. Only a location on this origin is returned.
    pub(super) fn resolve(&self, current: &str, location: &str) -> Option<Url> {
        let base = Url::parse(&self.base).ok()?;
        let target = Url::parse(current).ok()?.join(location).ok()?;
        let same_origin = target.scheme() == base.scheme()
            && target.host_str() == base.host_str()
            && target.port_or_known_default() == base.port_or_known_default();
        let anonymous = target.username().is_empty() && target.password().is_none();
        (same_origin && anonymous).then_some(target)
    }

    /// Sends one request and records the cookies the answer sets. Status classes are not
    /// judged here: the caller decides, with [`classify`], because some statuses carry data.
    pub(super) async fn exchange(
        &self,
        stage: &'static str,
        method: Method,
        url: &str,
        headers: Vec<(&'static str, String)>,
        body: Option<Vec<u8>>,
    ) -> Result<Reply, Failure> {
        let mut builder = api::browser_identity(self.http.request(method, url), USER_AGENT);
        for (name, value) in headers {
            builder = builder.header(name, value);
        }
        if let Some(cookie) = self.cookie_header() {
            builder = builder.header(COOKIE, cookie);
        }
        if let Some(body) = body {
            builder = builder.body(body);
        }
        let response = builder
            .send()
            .await
            .map_err(|_| Failure::new(Kind::Unavailable, stage, 0))?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        self.record_cookies(&headers);
        let body = response
            .bytes()
            .await
            .map_err(|_| Failure::new(Kind::BadResponse, stage, status))?;
        if body.len() > MAX_BODY {
            return Err(Failure::new(Kind::BadResponse, stage, status));
        }
        Ok(Reply {
            status,
            headers,
            body: body.to_vec(),
        })
    }

    /// The bootstrap: GET the login page, following same-origin redirects, and read the password
    /// action and the response headers it needs.
    pub(super) async fn bootstrap(&self) -> Result<(Page, HeaderMap), Failure> {
        let mut url = format!("{}{LOGIN_PATH}", self.base);
        for _ in 0..MAX_REDIRECTS {
            let reply = self
                .exchange(
                    "bootstrap",
                    Method::GET,
                    &url,
                    vec![("Accept", "text/html,application/xhtml+xml".to_owned())],
                    None,
                )
                .await?;
            if reply.is_redirect() {
                let location = reply
                    .location()
                    .and_then(|value| self.resolve(&url, value))
                    .ok_or_else(|| Failure::new(Kind::Blocked, "bootstrap", reply.status))?;
                let path = location.path();
                if path.starts_with("/checkpoint/") || path == "/authwall" {
                    return Err(self
                        .checkpoint_error(&location, "bootstrap", reply.status)
                        .await);
                }
                if path.starts_with("/zh-cn/customer/") {
                    return Err(Failure::new(Kind::RegionLocked, "bootstrap", reply.status));
                }
                url = location.to_string();
                continue;
            }
            if let Some(failure) = classify("bootstrap", &reply) {
                return Err(failure);
            }
            if reply.status != 200 {
                return Err(Failure::new(Kind::BadResponse, "bootstrap", reply.status));
            }
            let page = sdui::parse_page(&reply.body)
                .ok_or_else(|| Failure::new(Kind::Unsupported, "bootstrap", reply.status))?;
            return Ok((page, reply.headers));
        }
        Err(Failure::new(Kind::Blocked, "bootstrap", 0))
    }

    /// One password attempt: the bootstrap, then the submission of the identifier and password.
    pub(super) async fn login(
        &self,
        identifier: &str,
        password: &str,
    ) -> Result<Authenticated, Failure> {
        if identifier.trim().is_empty() || password.is_empty() {
            return Err(Failure::new(Kind::Rejected, "input", 0));
        }
        let (mut page, headers) = self.bootstrap().await?;
        if self.csrf().is_none() {
            return Err(Failure::new(Kind::Unsupported, "bootstrap_cookies", 0));
        }
        let leaf = header_str(&headers, "x-li-leaf-screen-id");
        page.set_screen(sdui::login_screen(leaf));
        let body = sdui::page_request(&page, identifier, password)
            .ok_or_else(|| Failure::new(Kind::Unsupported, "request", 0))?;
        self.submit(AUTH_REQUEST, body, &headers, "authenticate", LOGIN_PATH)
            .await
    }

    /// Posts a server request and follows an inline challenge route once, as the reference does.
    pub(super) async fn submit(
        &self,
        request_id: &'static str,
        body: Value,
        bootstrap: &HeaderMap,
        stage: &'static str,
        referer: &str,
    ) -> Result<Authenticated, Failure> {
        match self
            .post_action(request_id, &body, bootstrap, stage, referer)
            .await?
        {
            Post::Done(authenticated) => Ok(authenticated),
            Post::Route(route) => {
                let screen = sdui::login_screen(header_str(bootstrap, "x-li-leaf-screen-id"));
                let body = sdui::marshal_action(&route, Vec::new(), &screen);
                match self
                    .post_action(
                        CHALLENGE_ROUTE_REQUEST,
                        &body,
                        bootstrap,
                        "challenge_route",
                        referer,
                    )
                    .await?
                {
                    Post::Done(authenticated) => Ok(authenticated),
                    Post::Route(_) => Err(Failure::new(Kind::BadResponse, "completion", 0)),
                }
            }
        }
    }

    /// Posts one server request with the tracking headers of the bootstrap.
    async fn post_action(
        &self,
        request_id: &'static str,
        body: &Value,
        bootstrap: &HeaderMap,
        stage: &'static str,
        referer: &str,
    ) -> Result<Post, Failure> {
        let version = header_str(bootstrap, "x-li-application-version")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Failure::new(Kind::Unsupported, "bootstrap_headers", 0))?;
        let tracking = header_str(bootstrap, "x-li-page-instance-tracking-id")
            .filter(|value| !value.is_empty())
            .map_or_else(random_tracking_id, str::to_owned);
        let track = json!({
            "clientVersion": version,
            "mpVersion": version,
            "mpName": "web",
            "osName": "web",
            "deviceFormFactor": "DESKTOP",
            "displayDensity": 1,
            "displayWidth": 1920,
            "displayHeight": 1080,
        });
        let url = format!(
            "{}/flagship-web/rsc-action/actions/server-request?sduiid={request_id}",
            self.base
        );
        let payload =
            serde_json::to_vec(body).map_err(|_| Failure::new(Kind::Unsupported, stage, 0))?;
        let headers = vec![
            ("Accept", "*/*".to_owned()),
            ("Content-Type", "application/json".to_owned()),
            ("Origin", self.base.to_string()),
            ("Referer", format!("{}{referer}", self.base)),
            ("Csrf-Token", self.csrf().unwrap_or_default()),
            ("X-Li-Rsc-Stream", "true".to_owned()),
            ("X-Li-Application-Version", version.to_owned()),
            (
                "X-Li-Application-Instance",
                header_str(bootstrap, "x-li-application-instance")
                    .unwrap_or_default()
                    .to_owned(),
            ),
            ("X-Li-Page-Instance-Tracking-Id", tracking.clone()),
            ("X-Li-Anchor-Page-Key", "d_flagship3_login".to_owned()),
            (
                "X-Li-Page-Instance",
                format!("urn:li:page:d_flagship3_login;{tracking}"),
            ),
            ("X-Li-Track", track.to_string()),
            ("Sec-Fetch-Dest", "empty".to_owned()),
            ("Sec-Fetch-Mode", "cors".to_owned()),
            ("Sec-Fetch-Site", "same-origin".to_owned()),
        ];
        let reply = self
            .exchange(stage, Method::POST, &url, headers, Some(payload))
            .await?;
        if reply.is_redirect() {
            let checkpoint = reply
                .location()
                .and_then(|value| self.resolve(&url, value))
                .filter(|location| location.path().starts_with("/checkpoint/"));
            return Err(match checkpoint {
                Some(location) => self.checkpoint_error(&location, stage, reply.status).await,
                None => Failure::new(Kind::Challenge, stage, reply.status),
            });
        }
        if let Some(failure) = classify(stage, &reply) {
            return Err(failure);
        }
        let response = sdui::parse_response(&reply.body)
            .ok_or_else(|| Failure::new(Kind::BadResponse, stage, reply.status))?;
        if response.has_errors || response.inline_error {
            return Err(Failure::new(Kind::Rejected, stage, reply.status));
        }
        for raw in &response.completion_urls {
            let Some(location) = self.resolve(&url, raw) else {
                continue;
            };
            let path = location.path();
            if path.starts_with("/checkpoint/") || path == "/authwall" {
                return Err(self.checkpoint_error(&location, stage, reply.status).await);
            }
            if (path == "/feed/" || path == "/feed") && self.has_session() {
                return Ok(Post::Done(self.authenticated()));
            }
        }
        if let Some(route) = response
            .challenge_route
            .filter(|_| request_id != CHALLENGE_ROUTE_REQUEST)
        {
            return Ok(Post::Route(route));
        }
        Err(Failure::new(Kind::BadResponse, "completion", reply.status))
    }

    /// The checkpoint a redirect leads to. Its page is loaded now, so the next step can show it;
    /// a failed load leaves the checkpoint without a form, which the caller turns into a browser
    /// fallback, as the reference does.
    async fn checkpoint_error(&self, location: &Url, stage: &'static str, status: u16) -> Failure {
        let mut checkpoint = Checkpoint::new(self.clone(), request_uri(location));
        let _ = checkpoint.load().await;
        Failure {
            kind: Kind::Challenge,
            stage,
            status,
            retry_after_ms: 0,
            attached: Attached::New(Box::new(checkpoint)),
        }
    }

    fn cookie(&self, name: &str) -> Option<String> {
        self.jar_guard()
            .get(name)
            .filter(|value| !value.is_empty())
            .cloned()
    }

    fn cookie_header(&self) -> Option<String> {
        let jar = self.jar_guard();
        if jar.is_empty() {
            return None;
        }
        Some(
            jar.iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join("; "),
        )
    }

    fn record_cookies(&self, headers: &HeaderMap) {
        let mut jar = self.jar_guard();
        for value in headers.get_all(SET_COOKIE) {
            let Ok(text) = value.to_str() else {
                continue;
            };
            let pair = text.split(';').next().unwrap_or_default();
            if let Some((name, value)) = pair.split_once('=') {
                let name = name.trim();
                if !name.is_empty() {
                    jar.insert(name.to_owned(), value.trim().to_owned());
                }
            }
        }
    }

    fn jar_guard(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, String>> {
        self.jar.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Maps a status onto the reference's error classes. 3xx and 2xx are not errors.
pub(super) fn classify(stage: &'static str, reply: &Reply) -> Option<Failure> {
    let kind = match reply.status {
        429 => Kind::RateLimited,
        403 | 999 => Kind::Blocked,
        500..=599 => Kind::Unavailable,
        400..=499 => Kind::Rejected,
        _ => return None,
    };
    let mut failure = Failure::new(kind, stage, reply.status);
    if kind == Kind::RateLimited {
        failure.retry_after_ms = api::retry_after_ms(&reply.headers);
    }
    Some(failure)
}

/// The path and query of a URL, as a request target.
pub(super) fn request_uri(url: &Url) -> String {
    match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_owned(),
    }
}

/// A header value as text, when it is present and valid.
fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// The tracking id the web client sends when the page did not: 16 random bytes, base64.
fn random_tracking_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    STANDARD.encode(bytes)
}
