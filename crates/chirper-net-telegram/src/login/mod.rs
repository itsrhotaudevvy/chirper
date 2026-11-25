//! The three login flows: `phone` (code, then two-step password), `qr` (scan a login token) and
//! `bot` (bot token). Step ids and field ids are the Go bridge's, so the UI sees the same steps.
//!
//! Each flow is a [`LoginProcess`] over an [`Api`], so the state machines are tested against an
//! in-memory API. Telegram does not allow sign-up through a client, so a phone number without an
//! account is refused rather than registered.

use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use chirper_bridge::model::InputField;
use chirper_bridge::{Error, LoginCtx, LoginInput, LoginProcess, LoginStep, Result};
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::Credentials;
use crate::client::{Api, Incoming};
use crate::error::CallError;
use crate::mtproto::crypto;
use crate::srp;
use crate::tl::enums;
use crate::tl::functions;
use crate::tl::types;

/// Flow ids.
pub const FLOW_PHONE: &str = "phone";
pub const FLOW_QR: &str = "qr";
pub const FLOW_BOT: &str = "bot";

/// Step ids (the Go bridge's `fi.mau.telegram.login.*`).
pub const STEP_PHONE_NUMBER: &str = "fi.mau.telegram.login.phone_number";
pub const STEP_CODE: &str = "fi.mau.telegram.login.code";
pub const STEP_CODE_INCORRECT: &str = "fi.mau.telegram.login.code.incorrect";
pub const STEP_PASSWORD: &str = "fi.mau.telegram.login.password";
pub const STEP_PASSWORD_INCORRECT: &str = "fi.mau.telegram.login.password.incorrect";
pub const STEP_SHOW_QR: &str = "fi.mau.telegram.login.show_qr";
pub const STEP_BOT_TOKEN: &str = "fi.mau.telegram.login.bot_token";

/// Constructor id of `updateLoginToken` (`#564fe691`).
const UPDATE_LOGIN_TOKEN_ID: u32 = 0x564f_e691;

/// The data centres a login may have to try, so their auth keys can be forgotten on cancel.
const KNOWN_DCS: std::ops::RangeInclusive<i32> = 1..=5;

fn user_input(step_id: &str, instructions: &str, fields: Vec<InputField>) -> LoginStep {
    LoginStep::UserInput {
        step_id: step_id.to_owned(),
        instructions: instructions.to_owned(),
        fields,
    }
}

fn field(id: &str, name: &str, kind: &str, description: &str) -> InputField {
    InputField {
        id: id.to_owned(),
        name: name.to_owned(),
        kind: kind.to_owned(),
        description: description.to_owned(),
        ..InputField::default()
    }
}

fn phone_step() -> LoginStep {
    user_input(
        STEP_PHONE_NUMBER,
        "",
        vec![field(
            STEP_PHONE_NUMBER,
            "Phone number",
            "phone",
            "Include the country code with +",
        )],
    )
}

fn password_step(incorrect: bool) -> LoginStep {
    let (step, instructions) = if incorrect {
        (
            STEP_PASSWORD_INCORRECT,
            "Incorrect password, please try again. Use the official Telegram app to reset your password if you've forgotten it.",
        )
    } else {
        (STEP_PASSWORD, "You have two-factor authentication enabled.")
    };
    user_input(
        step,
        instructions,
        vec![field(STEP_PASSWORD, "Password", "password", "")],
    )
}

/// The code step; `description` says where the code was sent.
fn code_step(description: &str, incorrect: bool) -> LoginStep {
    if incorrect {
        return user_input(
            STEP_CODE_INCORRECT,
            "Incorrect code",
            vec![field(STEP_CODE, "Code", "number", "")],
        );
    }
    user_input(
        STEP_CODE,
        "Enter the login code Telegram sent you",
        vec![field(STEP_CODE, "Code", "number", description)],
    )
}

fn qr_step(token: &[u8]) -> LoginStep {
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token);
    LoginStep::DisplayAndWait {
        step_id: STEP_SHOW_QR.to_owned(),
        instructions: "Scan the QR code on your phone to log in".to_owned(),
        kind: "qr".to_owned(),
        data: format!("tg://login?token={encoded}"),
        image_url: String::new(),
    }
}

fn bot_token_step() -> LoginStep {
    LoginStep::UserInput {
        step_id: STEP_BOT_TOKEN.to_owned(),
        instructions: "Please enter the bot token you want to log in as".to_owned(),
        fields: vec![InputField {
            pattern: r"^\d+:[A-Za-z0-9_-]{35}$".to_owned(),
            ..field(STEP_BOT_TOKEN, "Bot token", "token", "")
        }],
    }
}

fn complete(login_id: i64) -> LoginStep {
    LoginStep::Complete {
        login_id: crate::ids::login_id(login_id),
    }
}

/// The message the user sees for a refused login call.
fn refused(error: CallError) -> Error {
    let message = match &error {
        CallError::Rpc { name, .. } => match name.as_str() {
            "PHONE_NUMBER_INVALID" => Some("this phone number is not valid"),
            "PHONE_NUMBER_BANNED" => Some("this phone number is banned"),
            "PHONE_CODE_EXPIRED" => Some("the login code expired, start the login again"),
            "PHONE_CODE_EMPTY" => Some("enter the login code"),
            "ACCESS_TOKEN_INVALID" | "BOT_INVALID" => {
                Some("Telegram does not accept this bot token")
            }
            "LOGIN_TOKEN_EXPIRED" => Some("the QR code expired, start the login again"),
            _ => None,
        },
        _ => None,
    };
    match message {
        Some(text) => Error::Protocol(text.to_owned()),
        None => error.into(),
    }
}

/// Telegram's description of where the login code went.
fn code_description(code_type: &enums::auth::SentCodeType) -> &'static str {
    match code_type {
        enums::auth::SentCodeType::SentCodeTypeSms(_)
        | enums::auth::SentCodeType::SentCodeTypeSmsWord(_)
        | enums::auth::SentCodeType::SentCodeTypeSmsPhrase(_)
        | enums::auth::SentCodeType::SentCodeTypeFirebaseSms(_) => {
            "The code was sent to your phone via SMS"
        }
        enums::auth::SentCodeType::SentCodeTypeCall(_) => {
            "The code will be read to you in a phone call"
        }
        enums::auth::SentCodeType::SentCodeTypeEmailCode(_) => "The code was sent to your email",
        enums::auth::SentCodeType::SentCodeTypeFragmentSms(_) => {
            "The code was sent to your Fragment account"
        }
        _ => "The code was sent to the Telegram app on your phone",
    }
}

/// `auth.Authorization` to the account's user id. Sign-up is refused.
fn authorized_user(authorization: enums::auth::Authorization) -> Result<i64> {
    match authorization {
        enums::auth::Authorization::Authorization(auth) => match *auth.user {
            enums::User::User(user) => Ok(user.id),
            enums::User::UserEmpty(_) => Err(Error::Protocol("Telegram returned no user".into())),
        },
        enums::auth::Authorization::AuthorizationSignUpRequired(_) => Err(Error::Auth(
            "this phone number has no Telegram account".into(),
        )),
    }
}

/// Answer a password challenge: fetch the current SRP parameters and check the password.
async fn check_password<A: Api>(
    api: &A,
    password: &str,
) -> std::result::Result<enums::auth::Authorization, CallError> {
    let account = api.call(&functions::account::GetPassword {}).await?;
    let enums::account::Password::Password(account) = account;
    let algo = account
        .current_algo
        .ok_or_else(|| CallError::Protocol("this account has no password".to_owned()))?;
    let enums::PasswordKdfAlgo::PasswordKdfAlgoSHA256SHA256PBKDF2HMACSHA512iter100000SHA256ModPow(
        algo,
    ) = *algo
    else {
        return Err(CallError::Protocol(
            "unsupported password algorithm".to_owned(),
        ));
    };
    let srp_b = account
        .srp_B
        .ok_or_else(|| CallError::Protocol("the password challenge is missing".to_owned()))?;
    let srp_id = account
        .srp_id
        .ok_or_else(|| CallError::Protocol("the password challenge is missing".to_owned()))?;
    let random = crypto::random_array::<256>().map_err(|e| CallError::Protocol(e.to_string()))?;
    let proof = srp::prove(
        password.as_bytes(),
        srp::Params {
            salt1: &algo.salt1,
            salt2: &algo.salt2,
            g: algo.g,
            p: &algo.p,
        },
        &srp_b,
        &random,
    )
    .map_err(|e| CallError::Protocol(e.to_string()))?;
    let input = enums::InputCheckPasswordSRP::InputCheckPasswordSRP(types::InputCheckPasswordSRP {
        srp_id,
        A: proof.a,
        M1: proof.m1,
    });
    api.call(&functions::auth::CheckPassword {
        password: Box::new(input),
    })
    .await
}

/// True when a password check failed because the password is wrong.
fn wrong_password(error: &CallError) -> bool {
    error.is_named("PASSWORD_HASH_INVALID")
}

/// The phone login: a number, the code Telegram sends, and the password when two-step
/// verification is on.
pub struct PhoneLogin<A: Api> {
    api: A,
    ctx: LoginCtx,
    credentials: Credentials,
    phase: PhonePhase,
}

#[derive(Clone, Debug)]
enum PhonePhase {
    Number,
    Code { phone: String, hash: String },
    Password { phone: String },
}

impl<A: Api> PhoneLogin<A> {
    /// A phone login over `api`, with the application credentials.
    pub fn new(api: A, ctx: LoginCtx, credentials: Credentials) -> Self {
        Self {
            api,
            ctx,
            credentials,
            phase: PhonePhase::Number,
        }
    }

    async fn send_code(&mut self, phone: String) -> Result<LoginStep> {
        let settings = enums::CodeSettings::CodeSettings(types::CodeSettings {
            allow_flashcall: false,
            current_number: false,
            allow_app_hash: false,
            allow_missed_call: false,
            allow_firebase: false,
            unknown_number: false,
            logout_tokens: None,
            token: None,
            app_sandbox: None,
        });
        let request = functions::auth::SendCode {
            phone_number: phone.clone(),
            api_id: self.credentials.api_id,
            api_hash: self.credentials.api_hash.clone(),
            settings: Box::new(settings),
        };
        match self.api.call(&request).await.map_err(refused)? {
            enums::auth::SentCode::SentCode(sent) => {
                if matches!(
                    *sent.r#type,
                    enums::auth::SentCodeType::SentCodeTypeSetUpEmailRequired(_)
                ) {
                    return Err(Error::Protocol(
                        "Telegram requires setting up a login email, please log in with a QR code instead"
                            .into(),
                    ));
                }
                let description = code_description(&sent.r#type);
                self.phase = PhonePhase::Code {
                    phone,
                    hash: sent.phone_code_hash,
                };
                Ok(code_step(description, false))
            }
            enums::auth::SentCode::SentCodeSuccess(success) => {
                let user = authorized_user(*success.authorization)?;
                self.finish(user).await
            }
            enums::auth::SentCode::SentCodePaymentRequired(_) => Err(Error::Unsupported(
                "logging in with a payment-required code",
            )),
        }
    }

    async fn check_code(&mut self, phone: String, hash: String, code: String) -> Result<LoginStep> {
        if code.trim().is_empty() {
            self.phase = PhonePhase::Code { phone, hash };
            return Ok(code_step("", false));
        }
        let request = functions::auth::SignIn {
            phone_number: phone.clone(),
            phone_code_hash: hash.clone(),
            phone_code: Some(code.trim().to_owned()),
            email_verification: None,
        };
        match self.api.call(&request).await {
            Ok(authorization) => {
                let user = authorized_user(authorization)?;
                self.finish(user).await
            }
            Err(error) if error.is_named("SESSION_PASSWORD_NEEDED") => {
                self.phase = PhonePhase::Password { phone };
                Ok(password_step(false))
            }
            Err(error) if error.is_named("PHONE_CODE_INVALID") => {
                self.phase = PhonePhase::Code { phone, hash };
                Ok(code_step("", true))
            }
            Err(error) => Err(refused(error)),
        }
    }

    async fn check_two_step(&mut self, phone: String, password: String) -> Result<LoginStep> {
        if password.is_empty() {
            self.phase = PhonePhase::Password { phone };
            return Ok(password_step(false));
        }
        match check_password(&self.api, &password).await {
            Ok(authorization) => {
                let user = authorized_user(authorization)?;
                self.finish(user).await
            }
            Err(error) if wrong_password(&error) => {
                self.phase = PhonePhase::Password { phone };
                Ok(password_step(true))
            }
            Err(error) => Err(refused(error)),
        }
    }

    async fn finish(&mut self, user: i64) -> Result<LoginStep> {
        self.phase = PhonePhase::Number;
        Ok(complete(user))
    }
}

#[async_trait]
impl<A: Api> LoginProcess for PhoneLogin<A> {
    async fn next(&mut self, input: LoginInput) -> Result<LoginStep> {
        let phase = self.phase.clone();
        match phase {
            PhonePhase::Number => {
                let phone = input
                    .values
                    .get(STEP_PHONE_NUMBER)
                    .map(|value| value.trim().to_owned())
                    .unwrap_or_default();
                if phone.is_empty() {
                    return Ok(phone_step());
                }
                self.send_code(phone).await
            }
            PhonePhase::Code { phone, hash } => {
                let code = input.values.get(STEP_CODE).cloned().unwrap_or_default();
                self.check_code(phone, hash, code).await
            }
            PhonePhase::Password { phone } => {
                let password = input.values.get(STEP_PASSWORD).cloned().unwrap_or_default();
                self.check_two_step(phone, password).await
            }
        }
    }

    async fn cancel(&mut self) {
        forget_provisional_keys(&self.ctx);
    }
}

/// Remove the auth keys and home data centre a cancelled login may have stored.
fn forget_provisional_keys(ctx: &LoginCtx) {
    for dc in KNOWN_DCS {
        let _ = ctx.delete_secret(&format!("auth_key/{dc}"));
    }
    let _ = ctx.delete_secret("home_dc");
}

/// The QR login: a login token shown as a QR code, re-issued as it expires, until a phone
/// approves it.
pub struct QrLogin<A: Api> {
    api: A,
    ctx: LoginCtx,
    credentials: Credentials,
    /// Server pushes of the home connection (`updateLoginToken` arrives here).
    frames: mpsc::UnboundedReceiver<Incoming>,
    phase: QrPhase,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum QrPhase {
    Start,
    /// Waiting for a scan; `token` is the one on screen, `expires` its Unix expiry.
    Waiting {
        token: Vec<u8>,
        expires: i32,
    },
    Password,
}

impl<A: Api> QrLogin<A> {
    /// A QR login over `api`, reading server pushes from `frames`.
    pub fn new(
        api: A,
        ctx: LoginCtx,
        credentials: Credentials,
        frames: mpsc::UnboundedReceiver<Incoming>,
    ) -> Self {
        Self {
            api,
            ctx,
            credentials,
            frames,
            phase: QrPhase::Start,
        }
    }

    /// Ask for a login token and show it, unless it is the one already on screen.
    async fn export(&mut self, previous: Option<&[u8]>) -> Result<Option<LoginStep>> {
        let request = functions::auth::ExportLoginToken {
            api_id: self.credentials.api_id,
            api_hash: self.credentials.api_hash.clone(),
            except_ids: Vec::new(),
        };
        match self.api.call(&request).await {
            Ok(enums::auth::LoginToken::LoginToken(token)) => {
                if previous == Some(token.token.as_slice()) {
                    self.phase = QrPhase::Waiting {
                        token: token.token,
                        expires: token.expires,
                    };
                    return Ok(None);
                }
                let step = qr_step(&token.token);
                self.phase = QrPhase::Waiting {
                    token: token.token,
                    expires: token.expires,
                };
                Ok(Some(step))
            }
            Ok(enums::auth::LoginToken::LoginTokenMigrateTo(moved)) => {
                let import = functions::auth::ImportLoginToken { token: moved.token };
                let authorization = self
                    .api
                    .call_on(moved.dc_id, &import)
                    .await
                    .map_err(refused)?;
                self.api.set_home_dc(moved.dc_id);
                let user = match authorization {
                    enums::auth::LoginToken::LoginTokenSuccess(success) => {
                        authorized_user(*success.authorization)?
                    }
                    _ => return Err(Error::Protocol("the QR login did not complete".into())),
                };
                self.finish(user).await.map(Some)
            }
            Ok(enums::auth::LoginToken::LoginTokenSuccess(success)) => {
                let user = authorized_user(*success.authorization)?;
                self.finish(user).await.map(Some)
            }
            Err(error) if error.is_named("SESSION_PASSWORD_NEEDED") => {
                self.phase = QrPhase::Password;
                Ok(Some(password_step(false)))
            }
            Err(error) => Err(refused(error)),
        }
    }

    /// Block until the token is scanned, or it expires and a new one is issued, and return the
    /// step to show next. The step is only returned when it changes what the user sees.
    async fn wait_for_change(&mut self) -> Result<LoginStep> {
        loop {
            let QrPhase::Waiting { token, expires } = self.phase.clone() else {
                return Err(Error::Protocol("the QR login is not waiting".into()));
            };
            let remaining = u64::try_from(i64::from(expires) - unix_now())
                .unwrap_or(0)
                .max(1);
            let timeout = tokio::time::sleep(Duration::from_secs(remaining));
            tokio::pin!(timeout);
            let changed = tokio::select! {
                frame = self.frames.recv() => match frame {
                    None | Some(Incoming::Closed { .. }) => {
                        return Err(Error::Network("the login connection closed".into()));
                    }
                    Some(Incoming::Frame { frame, .. }) => is_login_token_push(&frame),
                },
                () = &mut timeout => true,
            };
            if changed && let Some(step) = self.export(Some(&token)).await? {
                return Ok(step);
            }
        }
    }

    async fn check_two_step(&mut self, password: String) -> Result<LoginStep> {
        if password.is_empty() {
            return Ok(password_step(false));
        }
        match check_password(&self.api, &password).await {
            Ok(authorization) => {
                let user = authorized_user(authorization)?;
                self.finish(user).await
            }
            Err(error) if wrong_password(&error) => Ok(password_step(true)),
            Err(error) => Err(refused(error)),
        }
    }

    async fn finish(&mut self, user: i64) -> Result<LoginStep> {
        self.phase = QrPhase::Start;
        Ok(complete(user))
    }
}

/// True for the server push that says a login token was scanned.
fn is_login_token_push(frame: &crate::mtproto::UpdateFrame) -> bool {
    matches!(
        frame,
        crate::mtproto::UpdateFrame::Raw { constructor, .. } if *constructor == UPDATE_LOGIN_TOKEN_ID
    )
}

/// Seconds since the Unix epoch.
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[async_trait]
impl<A: Api> LoginProcess for QrLogin<A> {
    async fn next(&mut self, input: LoginInput) -> Result<LoginStep> {
        match self.phase.clone() {
            QrPhase::Start => match self.export(None).await? {
                Some(step) => Ok(step),
                None => Err(Error::Protocol("Telegram issued no login token".into())),
            },
            QrPhase::Waiting { .. } => self.wait_for_change().await,
            QrPhase::Password => {
                let password = input.values.get(STEP_PASSWORD).cloned().unwrap_or_default();
                self.check_two_step(password).await
            }
        }
    }

    async fn cancel(&mut self) {
        forget_provisional_keys(&self.ctx);
    }
}

/// The bot login: a bot token, logged out of the cloud Bot API first, then imported.
pub struct BotLogin<A: Api> {
    api: A,
    ctx: LoginCtx,
    credentials: Credentials,
}

#[derive(Deserialize)]
struct BotApiReply {
    ok: bool,
    #[serde(default)]
    error_code: i64,
    #[serde(default)]
    description: String,
}

impl<A: Api> BotLogin<A> {
    /// A bot login over `api`, with the application credentials.
    pub fn new(api: A, ctx: LoginCtx, credentials: Credentials) -> Self {
        Self {
            api,
            ctx,
            credentials,
        }
    }

    /// Log the token out of the cloud Bot API, which must happen before MTProto accepts it.
    /// Errors never include the URL, which carries the token.
    async fn logout_bot_api(&self, token: &str) -> Result<()> {
        let url = format!("https://api.telegram.org/bot{token}/logOut");
        let response = self
            .ctx
            .http
            .post(url)
            .send()
            .await
            .map_err(|_| Error::Network("could not reach the Bot API".into()))?;
        let reply: BotApiReply = response
            .json()
            .await
            .map_err(|_| Error::Protocol("the Bot API answered something unexpected".into()))?;
        if reply.ok || reply.description == "Logged out" {
            return Ok(());
        }
        if reply.error_code == 401 || reply.error_code == 404 {
            return Err(Error::Protocol(
                "Telegram does not accept this bot token".into(),
            ));
        }
        Err(Error::Protocol(format!(
            "Bot API refused the logout ({}): {}",
            reply.error_code, reply.description
        )))
    }
}

#[async_trait]
impl<A: Api> LoginProcess for BotLogin<A> {
    async fn next(&mut self, input: LoginInput) -> Result<LoginStep> {
        let Some(token) = input
            .values
            .get(STEP_BOT_TOKEN)
            .map(|t| t.trim().to_owned())
        else {
            return Ok(bot_token_step());
        };
        if token.is_empty() {
            return Ok(bot_token_step());
        }
        self.logout_bot_api(&token).await?;
        let request = functions::auth::ImportBotAuthorization {
            flags: 0,
            api_id: self.credentials.api_id,
            api_hash: self.credentials.api_hash.clone(),
            bot_auth_token: token,
        };
        let authorization = self.api.call(&request).await.map_err(refused)?;
        let user = authorized_user(authorization)?;
        Ok(complete(user))
    }

    async fn cancel(&mut self) {
        forget_provisional_keys(&self.ctx);
    }
}

#[cfg(test)]
mod tests;
