//! The two LinkedIn login flows. The cookie flow shows linkedin.com in a web view and reads the
//! session cookies. The password flow ([`password`]) signs in with an e-mail or phone number and
//! a password, and falls back to the cookie flow when LinkedIn wants a browser. Both end in the
//! same place: [`complete_login`] checks the cookies against the profile endpoint and stores them.
//!
//! Reference: `connector/login.go` (`CookieLogin`) and `connector/login_password.go`.

mod checkpoint;
mod client;
mod encrypt;
mod html;
mod password;
mod sdui;

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::json;
use zeroize::Zeroizing;

use chirper_bridge::model::{CookieField, CookieSource, LoginStep};
use chirper_bridge::{Error, LoginCtx, LoginInput, LoginProcess, Result};

pub(crate) use password::PasswordLogin;

use crate::api::{Api, Cookies, USER_AGENT, WWW};
use crate::ids;
use crate::session::{SECRET_COOKIES, SECRET_SESSION};

/// The flow id of the cookie login.
pub const FLOW_COOKIES: &str = "cookies";
/// The flow id of the password login.
pub const FLOW_PASSWORD: &str = "password";
/// The id of the step that asks for the cookies.
const STEP_COOKIES: &str = "fi.mau.linkedin.login.enter_cookies";
/// The field that carries the session cookie.
const FIELD_SESSION: &str = "li_at";
/// The field that carries the CSRF cookie.
const FIELD_CSRF: &str = "JSESSIONID";
/// The field that carries the browser's user agent, read from the page.
const FIELD_USER_AGENT: &str = "user_agent";
/// What the cookies step asks for when it is the first step of the cookie flow.
const COOKIES_INSTRUCTIONS: &str = "Sign in to LinkedIn. Chirper reads the li_at and JSESSIONID cookies and \
    stores them in the system credential store.";

/// The cookies step on the login page: a web view, and the fields it must yield.
pub fn cookies_step() -> LoginStep {
    cookies_page(COOKIES_INSTRUCTIONS)
}

/// The cookies step with the given instructions. The password flow uses it when it hands over.
fn cookies_page(instructions: &str) -> LoginStep {
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
        instructions: instructions.to_owned(),
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
        let step = complete_login(&self.ctx, &self.base, Cookies::new(values), &user_agent).await?;
        self.done = true;
        Ok(step)
    }

    async fn cancel(&mut self) {
        let _ = self.ctx.delete_secret(SECRET_COOKIES);
        let _ = self.ctx.delete_secret(SECRET_SESSION);
    }
}

/// Checks the session cookies against the profile, then stores them and completes the login.
/// Shared by the cookie flow and the password flow, so both store the same things.
async fn complete_login(
    ctx: &LoginCtx,
    base: &str,
    cookies: Cookies,
    user_agent: &str,
) -> Result<LoginStep> {
    if !cookies.is_complete() {
        return Err(Error::Auth(format!(
            "the {FIELD_SESSION} and {FIELD_CSRF} cookies are required"
        )));
    }
    let api = Api::new(ctx.http.clone(), base, cookies.clone(), user_agent);
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
    store(ctx, &cookies, user_agent)?;
    Ok(LoginStep::Complete {
        login_id: ids::login_id(&user_id),
    })
}

/// Stores the cookie jar and the user agent under the login's secret names.
fn store(ctx: &LoginCtx, cookies: &Cookies, user_agent: &str) -> Result<()> {
    let jar: BTreeMap<String, String> = cookies.to_map();
    let stored = Zeroizing::new(
        serde_json::to_vec(&jar)
            .map_err(|_| Error::Internal("encoding the LinkedIn cookies failed".to_owned()))?,
    );
    ctx.set_secret(SECRET_COOKIES, &stored)?;
    let session = serde_json::to_vec(&json!({ "user_agent": user_agent }))
        .map_err(|_| Error::Internal("encoding the LinkedIn session failed".to_owned()))?;
    ctx.set_secret(SECRET_SESSION, &session)
}
