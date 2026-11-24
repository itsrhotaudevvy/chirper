//! The login flows that need no email round trip: a browser token and cookie (`token`), and a bot or
//! user token pasted by hand (`app`). The email flow lives in `login_email.rs`.
//!
//! Flow ids, step ids and field ids follow `connector/login-cookie.go` and `connector/login-app.go`
//! of the mautrix Slack bridge (AGPL-3.0). `token_extract.js` is the script that reads the token
//! from the Slack web client's local storage, copied from that file.

use async_trait::async_trait;
use serde_json::json;

use chirper_bridge::model::{CookieField, CookieSource, InputField, LoginStep};
use chirper_bridge::{Error, LoginCtx, LoginInput, LoginProcess, Result};

use crate::api::Api;
use crate::ids;

/// The flow id of the browser token login.
pub const TOKEN_FLOW: &str = "token";
/// The flow id of the bot or user token login.
pub const APP_FLOW: &str = "app";
/// The step that asks for a browser token and cookie.
pub const TOKEN_STEP: &str = "fi.mau.slack.login.enter_auth_token";
/// The step that asks for a bot or user token.
pub const APP_STEP: &str = "fi.mau.slack.login.enter_app_tokens";
/// The fallback step of the browser token login, for values typed by hand.
pub const MANUAL_TOKEN_STEP: &str = "fi.mau.slack.login.enter_auth_token_manual";

/// The page a browser token login opens.
pub const TOKEN_PAGE: &str = "https://app.slack.com/";
/// The source name of the token read by [`EXTRACT_TOKEN_JS`].
pub const TOKEN_SOURCE: &str = "fi.mau.slack.auth_token";

/// The script that waits for the web client to store its token and returns it.
pub const EXTRACT_TOKEN_JS: &str = include_str!("token_extract.js");

/// The cookie and token fields of a browser login, as the web client exposes them.
pub fn token_cookie_fields() -> Vec<CookieField> {
    vec![
        CookieField {
            id: "auth_token".to_owned(),
            required: true,
            sources: vec![CookieSource {
                kind: "javascript".to_owned(),
                name: TOKEN_SOURCE.to_owned(),
                cookie_domain: String::new(),
            }],
        },
        CookieField {
            id: "cookie_token".to_owned(),
            required: true,
            sources: vec![CookieSource {
                kind: "cookie".to_owned(),
                name: "d".to_owned(),
                cookie_domain: "slack.com".to_owned(),
            }],
        },
    ]
}

/// A browser login step on `url`, asking for the token and cookie of the web client.
pub fn token_cookies_step(step_id: &str, url: &str, instructions: &str) -> LoginStep {
    LoginStep::Cookies {
        step_id: step_id.to_owned(),
        instructions: instructions.to_owned(),
        url: url.to_owned(),
        user_agent: String::new(),
        fields: token_cookie_fields(),
        extract_js: EXTRACT_TOKEN_JS.to_owned(),
        wait_for_url_pattern: String::new(),
    }
}

/// Check a token with Slack and store it. The cookie, when there is one, is stored beside it. The
/// login id is `TEAM-USER`, the Go bridge's user login id.
pub async fn store_login(
    ctx: &LoginCtx,
    api_base: &str,
    token: &str,
    cookie: Option<&str>,
) -> Result<String> {
    let api = Api::new(ctx.http.clone(), api_base, token, cookie.map(str::to_owned));
    let who = api.auth_test().await?;
    if who.team_id.is_empty() || who.user_id.is_empty() {
        return Err(Error::Protocol("Slack named no team or user".to_owned()));
    }
    ctx.set_secret("token", token.as_bytes())?;
    if let Some(cookie) = cookie {
        let cookies = json!({ "d": cookie }).to_string();
        ctx.set_secret("cookies", cookies.as_bytes())?;
    }
    Ok(ids::user_login_id(&who.team_id, &who.user_id))
}

/// Whether a value looks like a browser token and a browser cookie.
pub fn is_browser_pair(token: &str, cookie: &str) -> bool {
    token.starts_with("xoxc-") && cookie.starts_with("xoxd-")
}

/// The browser token login. The first step opens the web client; if the page cannot give the
/// token, the user can paste both values instead.
pub struct TokenLogin {
    pub ctx: LoginCtx,
    pub api_base: String,
    pub done: bool,
    /// Whether the cookie step has been shown already.
    pub asked: bool,
}

impl TokenLogin {
    async fn submit(&mut self, token: &str, cookie: &str) -> Result<LoginStep> {
        let token = token.trim();
        let cookie = cookie.trim();
        if !is_browser_pair(token, cookie) {
            return Err(Error::Auth(
                "the token must start with xoxc- and the cookie with xoxd-".to_owned(),
            ));
        }
        let login_id = store_login(&self.ctx, &self.api_base, token, Some(cookie)).await?;
        self.done = true;
        Ok(LoginStep::Complete { login_id })
    }
}

#[async_trait]
impl LoginProcess for TokenLogin {
    async fn next(&mut self, input: LoginInput) -> Result<LoginStep> {
        if self.done {
            return Err(Error::Protocol("the login already finished".to_owned()));
        }
        if !input.cookies.is_empty() {
            let token = input.cookies.get("auth_token").cloned().unwrap_or_default();
            let cookie = input.cookies.get("cookie_token").cloned().unwrap_or_default();
            if token.is_empty() || cookie.is_empty() {
                return Ok(manual_step());
            }
            return self.submit(&token, &cookie).await;
        }
        if !input.values.is_empty() {
            let token = input.values.get("auth_token").cloned().unwrap_or_default();
            let cookie = input.values.get("cookie_token").cloned().unwrap_or_default();
            return self.submit(&token, &cookie).await;
        }
        if self.asked {
            return Ok(manual_step());
        }
        self.asked = true;
        Ok(token_cookies_step(
            TOKEN_STEP,
            TOKEN_PAGE,
            "Sign in to Slack in the window. When Slack's web client loads, the token and cookie are read \
             automatically. You can also enter both values by hand.",
        ))
    }

    async fn cancel(&mut self) {
        let _ = self.ctx.delete_secret("token");
        let _ = self.ctx.delete_secret("cookies");
    }
}

/// The fallback step: both values typed by hand.
fn manual_step() -> LoginStep {
    LoginStep::UserInput {
        step_id: MANUAL_TOKEN_STEP.to_owned(),
        instructions: "Enter the web client token (xoxc-…) and the cookie d (xoxd-…). Both are in the \
                       browser's developer tools."
            .to_owned(),
        fields: vec![
            input_field("auth_token", "Token", "password", "The xoxc- token of the web client."),
            input_field("cookie_token", "Cookie", "password", "The value of the d cookie (xoxd-…)."),
        ],
    }
}

/// The bot or user token login.
pub struct AppLogin {
    pub ctx: LoginCtx,
    pub api_base: String,
    pub done: bool,
}

#[async_trait]
impl LoginProcess for AppLogin {
    async fn next(&mut self, input: LoginInput) -> Result<LoginStep> {
        if self.done {
            return Err(Error::Protocol("the login already finished".to_owned()));
        }
        let Some(token) = input.values.get("bot_token").map(|t| t.trim().to_owned()) else {
            return Ok(LoginStep::UserInput {
                step_id: APP_STEP.to_owned(),
                instructions: "Enter a Slack bot token (xoxb-…) or a user token (xoxp-…). It is stored in the \
                               system credential store."
                    .to_owned(),
                fields: vec![input_field(
                    "bot_token",
                    "Bot token",
                    "password",
                    "A bot token starting with xoxb- or a user token starting with xoxp-.",
                )],
            });
        };
        if !(token.starts_with("xoxb-") || token.starts_with("xoxp-")) {
            return Err(Error::Auth(
                "the token must start with xoxb- or xoxp-".to_owned(),
            ));
        }
        let login_id = store_login(&self.ctx, &self.api_base, &token, None).await?;
        self.done = true;
        Ok(LoginStep::Complete { login_id })
    }

    async fn cancel(&mut self) {
        let _ = self.ctx.delete_secret("token");
    }
}

/// A text input field with the given id, name, kind and description.
pub fn input_field(id: &str, name: &str, kind: &str, description: &str) -> InputField {
    InputField {
        id: id.to_owned(),
        name: name.to_owned(),
        kind: kind.to_owned(),
        description: description.to_owned(),
        options: Vec::new(),
        pattern: String::new(),
    }
}
