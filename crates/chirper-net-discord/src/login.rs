//! The login flows that end in a stored Discord token: the browser flow (Discord's own sign-in
//! page in a web view, then the token its web app keeps), the user-token flow and the bot-token
//! flow. The QR flow is in [`crate::remoteauth`].
//!
//! Every flow ends in [`finish`]: the token is checked against `GET /users/@me`, the account type
//! must match the flow, and the token and a bot flag are stored under [`SECRET_TOKEN`] and
//! [`SECRET_BOT`]. `connect` reads both back.
//!
//! Reference: `connector/login.go` (`tokenLogin`, `finishLogin`) and `connector/login_browser.go`.

use async_trait::async_trait;
use log::debug;

use chirper_bridge::model::{CookieField, CookieSource, InputField, LoginStep};
use chirper_bridge::network::LoginInput;
use chirper_bridge::{Error, LoginCtx, LoginProcess, Result};

use crate::api::Api;
use crate::map;

/// The flow id of the browser login.
pub const FLOW_BROWSER: &str = "browser";
/// The flow id of the user-token login.
pub const FLOW_TOKEN: &str = "token";
/// The flow id of the bot-token login.
pub const FLOW_BOT: &str = "bot";

/// The secret that holds the token as the user gave it (a bot token may keep its `Bot ` prefix
/// when it was pasted into the user-token flow).
pub const SECRET_TOKEN: &str = "token";
/// The secret that marks a stored token as a bot token. Present, with the value `1`, only for bot
/// accounts. `connect` uses it to send the bot authorization and the bot gateway intents.
pub const SECRET_BOT: &str = "bot";

const BROWSER_STEP: &str = "fi.chirper.discord.login.browser";
const TOKEN_STEP: &str = "fi.chirper.discord.login.enter_token";
const BOT_TOKEN_STEP: &str = "fi.chirper.discord.login.enter_bot_token";
/// The field that carries the token, in the input steps and on the cookies step.
const FIELD_TOKEN: &str = "token";
/// The page the browser flow opens.
const LOGIN_URL: &str = "https://discord.com/login";
/// The browser identity the login web view presents, as the Go connector did.
const LOGIN_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
    (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
/// The web app opens a channel once the sign-in is complete.
const DONE_PATTERN: &str = r"discord\.com/channels";
/// Reads the token the Discord web app keeps in local storage. The value is stored as a JSON
/// string; a value that is not a plausible token resolves to nothing, so the field stays missing.
const EXTRACT_TOKEN_JS: &str = r#"new Promise(resolve => {
  let token = null
  try {
    token = localStorage.getItem('token')
    try { token = JSON.parse(token) } catch (e) {}
  } catch (e) {}
  if (typeof token === 'string' && token.length >= 20) resolve({ token })
  else resolve({})
})"#;

/// The account type a flow's token must belong to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Account {
    /// A user account. A bot token is refused.
    User,
    /// A bot account, authorized with the `Bot ` prefix. A user token is refused.
    Bot,
    /// Either. The pasted prefix decides how the token authorizes (the user-token flow).
    Either,
}

/// The browser login: one cookies step on Discord's sign-in page, then the token it yields.
pub struct BrowserLogin {
    ctx: LoginCtx,
    base: String,
    done: bool,
}

impl BrowserLogin {
    /// A login that checks the token against `base`.
    pub fn new(ctx: LoginCtx, base: &str) -> Self {
        Self {
            ctx,
            base: base.to_owned(),
            done: false,
        }
    }
}

/// The cookies step: Discord's login page, and the token the web app keeps once it is signed in.
fn browser_step() -> LoginStep {
    LoginStep::Cookies {
        step_id: BROWSER_STEP.to_owned(),
        instructions:
            "Sign in to Discord. The window closes by itself as soon as you are logged in."
                .to_owned(),
        url: LOGIN_URL.to_owned(),
        user_agent: LOGIN_USER_AGENT.to_owned(),
        fields: vec![CookieField {
            id: FIELD_TOKEN.to_owned(),
            required: true,
            sources: vec![CookieSource {
                kind: "javascript".to_owned(),
                name: FIELD_TOKEN.to_owned(),
                cookie_domain: String::new(),
            }],
        }],
        extract_js: EXTRACT_TOKEN_JS.to_owned(),
        wait_for_url_pattern: DONE_PATTERN.to_owned(),
    }
}

#[async_trait]
impl LoginProcess for BrowserLogin {
    async fn next(&mut self, input: LoginInput) -> Result<LoginStep> {
        if self.done {
            return Err(Error::Protocol("the login already finished".to_owned()));
        }
        if input.cookies.is_empty() {
            return Ok(browser_step());
        }
        // The web app stores the token as a JSON string, so surrounding quotes are dropped.
        let token = input
            .cookies
            .get(FIELD_TOKEN)
            .map(|value| value.trim().trim_matches('"').trim())
            .filter(|token| !token.is_empty())
            .ok_or_else(|| {
                Error::Auth("please provide a valid Discord token to complete sign-in".to_owned())
            })?;
        let step = finish(&self.ctx, &self.base, token, Account::User).await?;
        self.done = true;
        Ok(step)
    }

    async fn cancel(&mut self) {
        forget(&self.ctx);
    }
}

/// The token login: one input step, then a live account. It serves the user-token flow and the
/// bot-token flow, which differ in the step they show and the account type they accept.
pub struct TokenLogin {
    ctx: LoginCtx,
    base: String,
    account: Account,
    done: bool,
}

impl TokenLogin {
    /// A login for `account` that checks the token against `base`.
    pub fn new(ctx: LoginCtx, base: &str, account: Account) -> Self {
        Self {
            ctx,
            base: base.to_owned(),
            account,
            done: false,
        }
    }

    fn input_step(&self) -> LoginStep {
        match self.account {
            Account::Bot => LoginStep::UserInput {
                step_id: BOT_TOKEN_STEP.to_owned(),
                instructions:
                    "Paste the token of your Discord bot application. It is stored in the \
                    system credential store."
                        .to_owned(),
                fields: vec![token_field(
                    "Bot token",
                    "The token of your Discord bot application.",
                )],
            },
            _ => LoginStep::UserInput {
                step_id: TOKEN_STEP.to_owned(),
                instructions:
                    "Paste your Discord token. It is stored in the system credential store."
                        .to_owned(),
                fields: vec![token_field(
                    "Token",
                    "A user token, or a bot token starting with `Bot `.",
                )],
            },
        }
    }
}

/// A password-kind input field for a token.
fn token_field(name: &str, description: &str) -> InputField {
    InputField {
        id: FIELD_TOKEN.to_owned(),
        name: name.to_owned(),
        kind: "password".to_owned(),
        description: description.to_owned(),
        ..InputField::default()
    }
}

#[async_trait]
impl LoginProcess for TokenLogin {
    async fn next(&mut self, input: LoginInput) -> Result<LoginStep> {
        if self.done {
            return Err(Error::Protocol("the login already finished".to_owned()));
        }
        if input.values.is_empty() {
            return Ok(self.input_step());
        }
        let pasted = input
            .values
            .get(FIELD_TOKEN)
            .map_or("", |value| value.trim());
        // Bot tokens come with the `Bot ` prefix the Developer Portal shows; it is added back
        // when the request is made, so it is not kept.
        let token = match self.account {
            Account::Bot => pasted.strip_prefix("Bot ").unwrap_or(pasted).trim(),
            _ => pasted,
        };
        if token.is_empty() {
            return Err(Error::Auth("no token was entered".to_owned()));
        }
        let step = finish(&self.ctx, &self.base, token, self.account).await?;
        self.done = true;
        Ok(step)
    }

    async fn cancel(&mut self) {
        forget(&self.ctx);
    }
}

/// Check a token against Discord, store it with its bot flag, and complete the login.
///
/// A bot token is authorized with the `Bot ` prefix, so it must be given without it. The token
/// is stored as given; the stored form authorizes the same way on connect.
pub async fn finish(
    ctx: &LoginCtx,
    base: &str,
    token: &str,
    account: Account,
) -> Result<LoginStep> {
    let auth = match account {
        Account::Bot => format!("Bot {token}"),
        Account::User | Account::Either => token.to_owned(),
    };
    let api = Api::new(ctx.http.clone(), &auth, base);
    let me = api.me().await.map_err(|err| match err {
        Error::Network(message) => Error::Network(message),
        _ => Error::Auth("Discord refused this token".to_owned()),
    })?;
    check_account(&me, account)?;
    ctx.set_secret(SECRET_TOKEN, token.as_bytes())?;
    if me.bot {
        ctx.set_secret(SECRET_BOT, b"1")?;
    } else {
        let _ = ctx.delete_secret(SECRET_BOT);
    }
    debug!("discord: logged in as {} (bot: {})", me.id, me.bot);
    Ok(LoginStep::Complete {
        login_id: format!("discord_{}", me.id),
    })
}

/// Refuse an account of the wrong type for the flow, as the Go connector did.
fn check_account(me: &map::User, account: Account) -> Result<()> {
    match (account, me.bot) {
        (Account::User, true) => Err(Error::Auth(
            "this token belongs to a bot account; log in with the bot token flow instead"
                .to_owned(),
        )),
        (Account::Bot, false) => Err(Error::Auth(
            "this token belongs to a user account; log in with the user token flow instead"
                .to_owned(),
        )),
        _ => Ok(()),
    }
}

/// Drop the secrets of a login that did not finish.
fn forget(ctx: &LoginCtx) {
    let _ = ctx.delete_secret(SECRET_TOKEN);
    let _ = ctx.delete_secret(SECRET_BOT);
}
