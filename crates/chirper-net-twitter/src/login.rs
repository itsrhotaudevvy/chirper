//! The login flows. `cookies` asks the user to sign in on x.com and hands over the session
//! cookies; `password` drives X's own sign-in pages (see [`password`]).
//! Reference: `connector/login.go` (`start`, `SubmitCookies`, `CreateLogin`).

use async_trait::async_trait;
use zeroize::Zeroizing;

use chirper_bridge::model::{CookieField, CookieSource, LoginStep};
use chirper_bridge::{Error, LoginCtx, LoginInput, LoginProcess, Result};

use crate::api::cookies::{AUTH_TOKEN, CT0, Cookies};
use crate::api::{Api, Bases, USER_AGENT, dm};
use crate::ids;
use crate::session::{SECRET_COOKIES, SECRET_SESSION};

/// The id of the cookie flow, pinned by the golden tests.
pub const FLOW_COOKIES: &str = "cookies";
/// The id of the password flow, pinned by the golden tests.
pub const FLOW_PASSWORD: &str = "password";
/// The step that asks for the cookies.
const STEP_COOKIES: &str = "fi.mau.twitter.login.enter_cookies";
/// The page the user signs in on.
const LOGIN_URL: &str = "https://x.com/i/flow/login";
/// The page X shows once the sign-in is complete.
const DONE_PATTERN: &str = r"^https://x\.com/home.*$";

/// The cookies step: a web view on the sign-in page, and the two cookies every request needs.
pub fn cookies_step() -> LoginStep {
    let field = |name: &str| CookieField {
        id: name.to_owned(),
        required: true,
        sources: vec![CookieSource {
            kind: "cookie".to_owned(),
            name: name.to_owned(),
            cookie_domain: "x.com".to_owned(),
        }],
    };
    LoginStep::Cookies {
        step_id: STEP_COOKIES.to_owned(),
        instructions: "Sign in to X. Chirper reads the auth_token and ct0 cookies and stores them \
            in the system credential store."
            .to_owned(),
        url: LOGIN_URL.to_owned(),
        user_agent: USER_AGENT.to_owned(),
        fields: vec![field(CT0), field(AUTH_TOKEN)],
        extract_js: String::new(),
        wait_for_url_pattern: DONE_PATTERN.to_owned(),
    }
}

/// The cookie login: one cookies step, then the cookies are checked against X and stored.
pub struct CookieLogin {
    ctx: LoginCtx,
    bases: Bases,
    done: bool,
}

impl CookieLogin {
    /// A login that checks its cookies against `bases`.
    pub fn new(ctx: LoginCtx, bases: Bases) -> Self {
        Self {
            ctx,
            bases,
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
        let cookies = Cookies::from_map(input.cookies);
        if !cookies.is_complete() {
            return Err(Error::Auth(format!(
                "the {AUTH_TOKEN} and {CT0} cookies are required"
            )));
        }
        let user_id = verify(&self.ctx, &cookies, &self.bases, None).await?;
        store_cookies(&self.ctx, &cookies, None)?;
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

/// Check cookies against X: the inbox must load, and the cookies must name a user. Returns the
/// user id.
pub async fn verify(
    ctx: &LoginCtx,
    cookies: &Cookies,
    bases: &Bases,
    user_agent: Option<&str>,
) -> Result<String> {
    let api = Api::new(ctx.http.clone(), cookies.clone(), bases.clone())
        .with_user_agent(user_agent.unwrap_or_default());
    let user_id = cookies
        .user_id()
        .ok_or_else(|| Error::Auth("the cookies do not name an X account".to_owned()))?;
    dm::inbox_initial_state(&api, false)
        .await
        .map_err(|err| match err {
            Error::Network(message) => Error::Network(message),
            _ => Error::Auth("X refused these cookies".to_owned()),
        })?;
    Ok(user_id)
}

/// Store the cookies (and the browser the session was made in) as the login's secrets.
pub fn store_cookies(ctx: &LoginCtx, cookies: &Cookies, user_agent: Option<&str>) -> Result<()> {
    let stored = Zeroizing::new(
        serde_json::to_vec(&cookies.to_map())
            .map_err(|_| Error::Internal("encoding the X cookies failed".to_owned()))?,
    );
    ctx.set_secret(SECRET_COOKIES, &stored)?;
    if let Some(user_agent) = user_agent {
        let session = serde_json::json!({ "user_agent": user_agent });
        ctx.set_secret(SECRET_SESSION, session.to_string().as_bytes())?;
    }
    Ok(())
}
