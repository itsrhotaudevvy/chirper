//! The cookie login: the user signs in on linkedin.com in a web view, and the `li_at` and
//! `JSESSIONID` cookies are handed over. Reference: `connector/login.go` (`CookieLogin`).
//!
//! The reference's password flow is not ported: it drives LinkedIn's server-rendered login pages
//! and falls back to a browser challenge, which the cookie flow covers.

use async_trait::async_trait;
use serde_json::json;
use zeroize::Zeroizing;

use chirper_bridge::model::{CookieField, CookieSource, LoginStep};
use chirper_bridge::{Error, LoginCtx, LoginInput, LoginProcess, Result};

use crate::api::{Api, Cookies, USER_AGENT, WWW};
use crate::ids;
use crate::session::{SECRET_COOKIES, SECRET_SESSION};

/// The flow id of the cookie login.
pub const FLOW_COOKIES: &str = "cookies";
/// The id of the step that asks for the cookies.
const STEP_COOKIES: &str = "fi.mau.linkedin.login.enter_cookies";
/// The field that carries the session cookie.
const FIELD_SESSION: &str = "li_at";
/// The field that carries the CSRF cookie.
const FIELD_CSRF: &str = "JSESSIONID";
/// The field that carries the browser's user agent, read from the page.
const FIELD_USER_AGENT: &str = "user_agent";

/// The cookies step: a web view on the login page, and the fields it must yield.
pub fn cookies_step() -> LoginStep {
    let cookie = |id: &str| CookieField {
        id: id.to_owned(),
        required: true,
        sources: vec![CookieSource {
            kind: "cookie".to_owned(),
            name: id.to_owned(),
            cookie_domain: "www.linkedin.com".to_owned(),
        }],
    };
    LoginStep::Cookies {
        step_id: STEP_COOKIES.to_owned(),
        instructions: "Sign in to LinkedIn. Chirper reads the li_at and JSESSIONID cookies and \
            stores them in the system credential store."
            .to_owned(),
        url: format!("{WWW}/login"),
        user_agent: USER_AGENT.to_owned(),
        fields: vec![
            cookie(FIELD_SESSION),
            cookie(FIELD_CSRF),
            CookieField {
                id: FIELD_USER_AGENT.to_owned(),
                required: false,
                sources: vec![CookieSource {
                    kind: "javascript".to_owned(),
                    name: FIELD_USER_AGENT.to_owned(),
                    cookie_domain: String::new(),
                }],
            },
        ],
        extract_js: format!(r#"Promise.resolve({{"{FIELD_USER_AGENT}": navigator.userAgent}})"#),
        wait_for_url_pattern: r"^https://www\.linkedin\.com/feed.*$".to_owned(),
    }
}

/// The cookie login: one cookies step, then the account is checked and stored.
pub struct CookieLogin {
    ctx: LoginCtx,
    base: String,
    done: bool,
}

impl CookieLogin {
    /// A login that checks its cookies against `base`.
    pub fn new(ctx: LoginCtx, base: &str) -> Self {
        Self {
            ctx,
            base: base.to_owned(),
            done: false,
        }
    }
}

#[async_trait]
impl LoginProcess for CookieLogin {
    async fn next(&mut self, input: LoginInput) -> Result<LoginStep> {
        if self.done {
            return Err(Error::Protocol("the login already finished".to_owned()));
        }
        if input.cookies.is_empty() {
            return Ok(cookies_step());
        }
        let mut values = input.cookies;
        let user_agent = values
            .remove(FIELD_USER_AGENT)
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| USER_AGENT.to_owned());
        let cookies = Cookies::new(values);
        if !cookies.is_complete() {
            return Err(Error::Auth(format!(
                "the {FIELD_SESSION} and {FIELD_CSRF} cookies are required"
            )));
        }
        let api = Api::new(
            self.ctx.http.clone(),
            &self.base,
            cookies.clone(),
            &user_agent,
        );
        let profile = api.profile().await.map_err(|error| match error {
            Error::Auth(_) => Error::Auth("LinkedIn refused these cookies".to_owned()),
            other => other,
        })?;
        let user_id = ids::user_id(&profile.mini.entity_urn);
        if user_id.is_empty() {
            return Err(Error::Auth(
                "LinkedIn returned no profile for these cookies".to_owned(),
            ));
        }
        let stored = Zeroizing::new(
            serde_json::to_vec(&cookies.to_map())
                .map_err(|_| Error::Internal("encoding the LinkedIn cookies failed".to_owned()))?,
        );
        self.ctx.set_secret(SECRET_COOKIES, &stored)?;
        let session = serde_json::to_vec(&json!({ "user_agent": user_agent }))
            .map_err(|_| Error::Internal("encoding the LinkedIn session failed".to_owned()))?;
        self.ctx.set_secret(SECRET_SESSION, &session)?;
        self.done = true;
        Ok(LoginStep::Complete {
            login_id: ids::login_id(&user_id),
        })
    }

    async fn cancel(&mut self) {
        let _ = self.ctx.delete_secret(SECRET_COOKIES);
        let _ = self.ctx.delete_secret(SECRET_SESSION);
    }
}
