//! The password login: an e-mail or phone number and a password, then whatever verification
//! LinkedIn asks for (a code, or an approval in the app). When LinkedIn wants a browser, the
//! login hands over to the cookie flow: the user finishes in the app's web view.
//!
//! Reference: `connector/login_password.go` (steps, error mapping) and `connector/loginerrors.go`.

use async_trait::async_trait;

use chirper_bridge::model::{InputField, LoginStep};
use chirper_bridge::{Error, LoginCtx, LoginInput, LoginProcess, Result};

use super::checkpoint::{Checkpoint, FormKind};
use super::client::{Attached, Authenticated, Client, Failure, Kind};
use super::{CookieLogin, complete_login, cookies_page};
use crate::api::{Cookies, USER_AGENT};

/// The step ids, as the Go build names them.
const STEP_CREDENTIALS: &str = "fi.mau.linkedin.login.credentials";
const STEP_EMAIL_CODE: &str = "fi.mau.linkedin.login.email_code";
const STEP_SMS_CODE: &str = "fi.mau.linkedin.login.sms_code";
const STEP_TOTP_CODE: &str = "fi.mau.linkedin.login.totp_code";
const STEP_APP_METHOD: &str = "fi.mau.linkedin.login.app_method";
const STEP_APP_APPROVAL: &str = "fi.mau.linkedin.login.app_approval";
/// The action a user picks on the app step.
const ACTION_APPROVE: &str = "Approve in LinkedIn app";
const ACTION_OTHER_WAY: &str = "Try another way";

/// The password login. One value per attempt: the checkpoint and the browser fallback are kept
/// until the login ends.
pub(crate) struct PasswordLogin {
    ctx: LoginCtx,
    base: String,
    /// The verification the login is waiting on, if any.
    checkpoint: Option<Checkpoint>,
    /// Set while the user approves in the LinkedIn app; the next call waits for the approval.
    awaiting_app: bool,
    /// The browser fallback, once the login has handed over.
    browser: Option<CookieLogin>,
    finished: bool,
}

impl PasswordLogin {
    /// A password login against `base`.
    pub(crate) fn new(ctx: LoginCtx, base: &str) -> Self {
        Self {
            ctx,
            base: base.to_owned(),
            checkpoint: None,
            awaiting_app: false,
            browser: None,
            finished: false,
        }
    }

    /// One step, given the input of the last one.
    async fn advance(&mut self, input: LoginInput) -> Result<LoginStep> {
        if let Some(browser) = self.browser.as_mut() {
            return browser.next(input).await;
        }
        if self.awaiting_app {
            self.awaiting_app = false;
            return self.wait_for_app().await;
        }
        if self.checkpoint.is_some() {
            return self.advance_checkpoint(&input).await;
        }
        self.advance_credentials(&input).await
    }

    /// The credentials step: the identifier and password go to LinkedIn.
    async fn advance_credentials(&mut self, input: &LoginInput) -> Result<LoginStep> {
        if input.values.is_empty() {
            return Ok(credentials_step(
                "Enter your LinkedIn email or phone number and password.",
            ));
        }
        let identifier = input
            .values
            .get("identifier")
            .map_or("", |value| value.trim());
        let password = input.values.get("password").map_or("", String::as_str);
        if identifier.is_empty() || password.is_empty() {
            return Ok(credentials_step(
                "Enter both your LinkedIn email or phone number and password.",
            ));
        }
        let client = Client::new(self.ctx.http.clone(), &self.base);
        match client.login(identifier, password).await {
            Ok(authenticated) => self.complete(authenticated).await,
            Err(failure) => self.handle_failure(failure),
        }
    }

    /// A step of the verification: a code, or the app method.
    async fn advance_checkpoint(&mut self, input: &LoginInput) -> Result<LoginStep> {
        let action = input.values.get("action").map_or("", String::as_str);
        let Some(checkpoint) = self.checkpoint.as_mut() else {
            return Err(Error::Protocol("no verification is pending".to_owned()));
        };
        if checkpoint.kind() == Some(FormKind::App) {
            if action == ACTION_APPROVE {
                self.awaiting_app = true;
                return Ok(app_approval_step());
            }
            if action == ACTION_OTHER_WAY && checkpoint.can_try_another_way() {
                let result = checkpoint.try_another_way().await;
                return self.finish_checkpoint(result).await;
            }
            return Ok(app_method_step(checkpoint.can_try_another_way()));
        }
        let code = input.values.get("code").map_or("", |value| value.trim());
        let result = checkpoint.submit_code(code).await;
        self.finish_checkpoint(result).await
    }

    /// Waits until the user approves in the app, then finishes the login.
    async fn wait_for_app(&mut self) -> Result<LoginStep> {
        let Some(checkpoint) = self.checkpoint.as_mut() else {
            return Err(Error::Protocol("no app approval is pending".to_owned()));
        };
        let result = checkpoint.wait_for_app().await;
        self.finish_checkpoint(result).await
    }

    async fn finish_checkpoint(
        &mut self,
        result: std::result::Result<Authenticated, Failure>,
    ) -> Result<LoginStep> {
        match result {
            Ok(authenticated) => {
                self.checkpoint = None;
                self.complete(authenticated).await
            }
            Err(failure) => self.handle_failure(failure),
        }
    }

    /// The login succeeded: the cookies are stored as the cookie flow stores them.
    async fn complete(&mut self, authenticated: Authenticated) -> Result<LoginStep> {
        self.checkpoint = None;
        let cookies = Cookies::new(authenticated.cookies);
        complete_login(&self.ctx, &self.base, cookies, USER_AGENT).await
    }

    /// Turns a failure into the next step, or into the error the user sees, as the Go build's
    /// `handleLoginError` does.
    fn handle_failure(&mut self, failure: Failure) -> Result<LoginStep> {
        match failure.kind {
            Kind::RateLimited => Err(Error::RateLimited {
                retry_after_ms: failure.retry_after_ms,
            }),
            Kind::Unavailable => Err(Error::Network(
                "LinkedIn couldn't be reached to finish signing in. Please try again.".to_owned(),
            )),
            Kind::Rejected => self.rejected(failure.stage),
            Kind::Challenge => self.challenge(failure.attached),
            _ => self.fallback(DEFAULT_FALLBACK),
        }
    }

    /// A rejected step: a code to retype, the credentials again, or the browser.
    fn rejected(&mut self, stage: &str) -> Result<LoginStep> {
        match stage {
            "email_code" => Ok(code_step(
                FormKind::Email,
                "LinkedIn did not accept that code. Check the latest verification email and try again.",
            )),
            "sms_code" => Ok(code_step(
                FormKind::Sms,
                "LinkedIn did not accept that code. Check the latest text message and try again.",
            )),
            "totp_code" => Ok(code_step(
                FormKind::Totp,
                "LinkedIn did not accept that code. Enter the current six-digit code from your authenticator app and try again.",
            )),
            "app_approval_expired" => self.fallback(
                "The LinkedIn app approval request expired. Try signing in again or continue through the browser.",
            ),
            "authenticate" | "input" => {
                self.checkpoint = None;
                Ok(credentials_step(
                    "LinkedIn rejected the sign-in. Check your details and try again, or choose the Cookies login method to sign in through a browser.",
                ))
            }
            "checkpoint_complete" => self.fallback(
                "LinkedIn verified your sign-in but could not finish logging in. Continue through the browser.",
            ),
            _ => self.fallback(DEFAULT_FALLBACK),
        }
    }

    /// A challenge: the step for the verification it asks for, or the browser when the page is
    /// one the driver does not know.
    fn challenge(&mut self, attached: Attached) -> Result<LoginStep> {
        match attached {
            Attached::New(checkpoint) => self.checkpoint = Some(*checkpoint),
            Attached::Current => {}
            Attached::Nothing => self.checkpoint = None,
        }
        let Some(checkpoint) = self.checkpoint.as_ref() else {
            return self.fallback("LinkedIn requires additional verification. Continue signing in through the browser.");
        };
        match checkpoint.kind() {
            Some(FormKind::Email) => Ok(code_step(
                FormKind::Email,
                "LinkedIn sent a verification code to your email address. Enter the six-digit code to finish signing in.",
            )),
            Some(FormKind::Sms) => Ok(code_step(
                FormKind::Sms,
                "LinkedIn sent a text message to your phone. Enter the six-digit code to finish signing in.",
            )),
            Some(FormKind::Totp) => Ok(code_step(
                FormKind::Totp,
                "Enter the current six-digit LinkedIn code from your authenticator app to finish signing in.",
            )),
            Some(FormKind::App) => Ok(app_method_step(checkpoint.can_try_another_way())),
            None => self.fallback("LinkedIn requires additional verification. Continue signing in through the browser."),
        }
    }

    /// Hands the login to the cookie flow. The step is a cookies step on the login page; the
    /// user finishes signing in there, and the cookies come back to [`CookieLogin`].
    fn fallback(&mut self, instructions: &str) -> Result<LoginStep> {
        self.checkpoint = None;
        self.browser = Some(CookieLogin::new(self.ctx.clone(), &self.base));
        Ok(cookies_page(instructions))
    }
}

#[async_trait]
impl LoginProcess for PasswordLogin {
    async fn next(&mut self, input: LoginInput) -> Result<LoginStep> {
        if self.finished {
            return Err(Error::Protocol("the login already finished".to_owned()));
        }
        let step = self.advance(input).await?;
        if matches!(step, LoginStep::Complete { .. }) {
            self.finished = true;
        }
        Ok(step)
    }

    async fn cancel(&mut self) {
        self.checkpoint = None;
        self.browser = None;
        self.awaiting_app = false;
    }
}

/// The fallback text when the failure has no more specific one.
const DEFAULT_FALLBACK: &str = "LinkedIn could not complete this sign-in directly. You can finish signing in through the browser.";

/// The e-mail or phone number and password step.
fn credentials_step(instructions: &str) -> LoginStep {
    LoginStep::UserInput {
        step_id: STEP_CREDENTIALS.to_owned(),
        instructions: instructions.to_owned(),
        fields: vec![
            InputField {
                id: "identifier".to_owned(),
                name: "Email or phone number".to_owned(),
                kind: "email".to_owned(),
                ..InputField::default()
            },
            InputField {
                id: "password".to_owned(),
                name: "Password".to_owned(),
                kind: "password".to_owned(),
                ..InputField::default()
            },
        ],
    }
}

/// The six-digit code step of one kind of verification.
fn code_step(kind: FormKind, instructions: &str) -> LoginStep {
    let (step_id, name) = match kind {
        FormKind::Sms => (STEP_SMS_CODE, "SMS verification code"),
        FormKind::Totp => (STEP_TOTP_CODE, "Authenticator app code"),
        FormKind::Email | FormKind::App => (STEP_EMAIL_CODE, "Email verification code"),
    };
    LoginStep::UserInput {
        step_id: step_id.to_owned(),
        instructions: instructions.to_owned(),
        fields: vec![InputField {
            id: "code".to_owned(),
            name: name.to_owned(),
            kind: "text".to_owned(),
            pattern: "^[0-9]{6}$".to_owned(),
            ..InputField::default()
        }],
    }
}

/// The choice on the app page: approve in the app, or try another way when it is offered.
fn app_method_step(can_try_another_way: bool) -> LoginStep {
    let mut options = vec![ACTION_APPROVE.to_owned()];
    if can_try_another_way {
        options.push(ACTION_OTHER_WAY.to_owned());
    }
    LoginStep::UserInput {
        step_id: STEP_APP_METHOD.to_owned(),
        instructions: "LinkedIn sent a sign-in request to your LinkedIn app. Approve it there, or choose another verification method.".to_owned(),
        fields: vec![InputField {
            id: "action".to_owned(),
            name: "Verification method".to_owned(),
            kind: "select".to_owned(),
            options,
            ..InputField::default()
        }],
    }
}

/// The step that waits for the approval in the app. The next call waits for it.
fn app_approval_step() -> LoginStep {
    LoginStep::DisplayAndWait {
        step_id: STEP_APP_APPROVAL.to_owned(),
        instructions: "Open the LinkedIn app on your phone and approve this sign-in. To use another verification method, go back.".to_owned(),
        kind: "nothing".to_owned(),
        data: String::new(),
        image_url: String::new(),
    }
}
