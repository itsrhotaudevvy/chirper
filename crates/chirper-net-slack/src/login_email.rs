//! The email login: address, CAPTCHA when Slack asks for one, confirmation code, workspace choice,
//! and two-factor or browser sign-in where a workspace needs it.
//!
//! A port of `connector/login-email.go` (mautrix-slack, AGPL-3.0). The state machine keeps the Go
//! flow's steps and their step and field ids: `fi.mau.slack.login.enter_email`,
//! `…email_captcha`, `…enter_email_code`, `…select_workspace`, `…two_factor`, `…workspace_browser`.
//! The CAPTCHA is shown in the app's web view: the step is a `Cookies` step on the sign-in page whose
//! `extract_js` is the reCAPTCHA script of `login-email-captcha.js`, and the solved token comes back
//! as the `captcha_token` field.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::json;

use chirper_bridge::model::{CookieField, CookieSource, LoginStep};
use chirper_bridge::{Error, LoginCtx, LoginInput, LoginProcess, Result};

use crate::Endpoints;
use crate::login::{self, input_field, store_login, token_cookies_step};
use crate::signin::{Signin, SigninError, SigninResult};
use crate::wire::Workspace;

/// The flow id of the email login.
pub const FLOW: &str = "email";
const EMAIL_STEP: &str = "fi.mau.slack.login.enter_email";
const CAPTCHA_STEP: &str = "fi.mau.slack.login.email_captcha";
const CODE_STEP: &str = "fi.mau.slack.login.enter_email_code";
const WORKSPACE_STEP: &str = "fi.mau.slack.login.select_workspace";
const TWO_FACTOR_STEP: &str = "fi.mau.slack.login.two_factor";
const BROWSER_STEP: &str = "fi.mau.slack.login.workspace_browser";
/// The sign-in page the CAPTCHA is shown on.
const CAPTCHA_PAGE: &str = "https://slack.com/signin";
/// The reCAPTCHA script that renders the challenge and returns its token.
const CAPTCHA_JS: &str = include_str!("captcha.js");

/// Where the email flow is.
enum State {
    /// Not started: the first call shows the address step.
    Start,
    Email,
    Captcha {
        email: String,
        site_key: String,
    },
    Code {
        email: String,
    },
    Workspace {
        options: Vec<(String, Workspace)>,
    },
    Browser {
        workspace: Workspace,
    },
    TwoFactor {
        workspace: Workspace,
    },
}

/// The email login process.
pub struct EmailLogin {
    ctx: LoginCtx,
    endpoints: Endpoints,
    signin: Signin,
    state: State,
    done: bool,
}

impl EmailLogin {
    /// A new email login. Nothing is sent until the first step is answered.
    pub fn new(ctx: LoginCtx, endpoints: Endpoints, http: reqwest::Client) -> Self {
        let signin = Signin::new(http, &endpoints.root, &endpoints.app);
        Self {
            ctx,
            endpoints,
            signin,
            state: State::Start,
            done: false,
        }
    }

    /// Move to the email step and return it.
    fn email(&mut self, instructions: &str) -> LoginStep {
        self.state = State::Email;
        email_step(instructions)
    }

    async fn submit_email(&mut self, input: &LoginInput) -> Result<LoginStep> {
        let email = input
            .values
            .get("email")
            .map(|value| value.trim().to_owned())
            .unwrap_or_default();
        if !valid_email(&email) {
            return Ok(self.email("Enter a valid Slack account email address."));
        }
        match self.request_code(&email).await {
            Ok(step) => Ok(step),
            Err(err) => match refusal_text(&err) {
                Some(text) => Ok(self.email(text)),
                None => Err(bridge_error(&err)),
            },
        }
    }

    /// Ask Slack for the code, through a CAPTCHA when it wants one.
    async fn request_code(&mut self, email: &str) -> SigninResult<LoginStep> {
        if self.signin.check_email(email).await? {
            let site_key = self.signin.captcha_site_key().await?;
            let step = captcha_step(
                &site_key,
                "Slack requires a CAPTCHA before it can email the confirmation code. Complete the embedded challenge to continue.",
            );
            self.state = State::Captcha {
                email: email.to_owned(),
                site_key,
            };
            return Ok(step);
        }
        self.signin.confirm_email(email, None).await?;
        self.state = State::Code {
            email: email.to_owned(),
        };
        Ok(code_step(
            "Slack emailed you a confirmation code. Enter the six characters from that email.",
        ))
    }

    async fn submit_captcha(
        &mut self,
        email: String,
        site_key: String,
        input: &LoginInput,
    ) -> Result<LoginStep> {
        if input.cookies.is_empty() {
            return Ok(self.captcha(
                &email,
                &site_key,
                "Complete the embedded CAPTCHA before continuing.",
            ));
        }
        let answer = input
            .cookies
            .get("captcha_token")
            .map(|value| value.trim().to_owned())
            .unwrap_or_default();
        if answer.is_empty() {
            return Ok(self.captcha(
                &email,
                &site_key,
                "The CAPTCHA did not return a solution. Complete a new embedded challenge to continue.",
            ));
        }
        match self.signin.confirm_email(&email, Some(&answer)).await {
            Ok(()) => {
                self.state = State::Code { email };
                Ok(code_step(
                    "Slack emailed you a confirmation code. Enter the six characters from that email.",
                ))
            }
            Err(SigninError::Refused { code, .. }) => match code.as_str() {
                "captcha_required" | "captcha_failed" | "invalid_captcha" => Ok(self.captcha(
                    &email,
                    &site_key,
                    "Slack rejected or expired that CAPTCHA solution. Complete a new embedded challenge to continue.",
                )),
                "ratelimited" | "rate_limited" | "code_request_limit_reached" => Ok(self.captcha(
                    &email,
                    &site_key,
                    "Slack is rate limiting email sign-in. Wait a few minutes, then complete a new embedded challenge.",
                )),
                "invalid_email" | "email_invalid" => {
                    Ok(self.email("Slack rejected that email address. Check it and try again."))
                }
                _ => Err(bridge_error(&SigninError::Refused { method: "signup.confirmEmail", code })),
            },
            Err(err) => Err(bridge_error(&err)),
        }
    }

    fn captcha(&mut self, email: &str, site_key: &str, instructions: &str) -> LoginStep {
        self.state = State::Captcha {
            email: email.to_owned(),
            site_key: site_key.to_owned(),
        };
        captcha_step(site_key, instructions)
    }

    async fn submit_code(&mut self, email: String, input: &LoginInput) -> Result<LoginStep> {
        let code = input
            .values
            .get("code")
            .map(|value| value.trim().to_uppercase())
            .unwrap_or_default();
        if !valid_code(&code) {
            self.state = State::Code { email };
            return Ok(code_step("Enter the six-character code Slack emailed you."));
        }
        let compact = code.replace('-', "");
        let found = async {
            self.signin.confirm_code(&email, &compact).await?;
            self.signin.find_workspaces().await
        }
        .await;
        let workspaces = match found {
            Ok(workspaces) => workspaces,
            Err(SigninError::Refused { code, .. }) => {
                return match code.as_str() {
                    "invalid_code" | "invalid_email_code" | "code_already_used" | "failed" => {
                        self.state = State::Code { email };
                        Ok(code_step("Slack rejected that confirmation code. Check the email and try again."))
                    }
                    "expired_code" | "code_expired" => Ok(self.email(
                        "That Slack confirmation code expired. Enter your email to request a new one.",
                    )),
                    "ratelimited" | "rate_limited" => {
                        self.state = State::Code { email };
                        Ok(code_step("Slack is rate limiting code checks. Wait before trying again."))
                    }
                    _ => Err(bridge_error(&SigninError::Refused {
                        method: "signin.confirmCode",
                        code,
                    })),
                };
            }
            Err(err) => return Err(bridge_error(&err)),
        };
        if workspaces.is_empty() {
            return Ok(self.email(
                "Slack did not return any workspaces that support email sign-in for this account.",
            ));
        }
        let options = label_workspaces(workspaces);
        if options.len() == 1 {
            let (_, workspace) = options[0].clone();
            return self.finish_workspace(workspace, options).await;
        }
        Ok(self.choose(options, "Choose the Slack workspace to connect."))
    }

    /// Show the workspace choice, keeping the options for the next answer.
    fn choose(&mut self, options: Vec<(String, Workspace)>, instructions: &str) -> LoginStep {
        let labels = options
            .iter()
            .map(|(label, _)| label.clone())
            .collect::<Vec<_>>();
        self.state = State::Workspace { options };
        workspace_step(labels, instructions)
    }

    async fn submit_workspace(
        &mut self,
        options: Vec<(String, Workspace)>,
        input: &LoginInput,
    ) -> Result<LoginStep> {
        let selection = input
            .values
            .get("workspace")
            .map(|value| value.trim().to_owned())
            .unwrap_or_default();
        match options
            .iter()
            .find(|(label, _)| *label == selection)
            .cloned()
        {
            Some((_, workspace)) => self.finish_workspace(workspace, options).await,
            None => Ok(self.choose(options, "Choose one of the Slack workspaces in the list.")),
        }
    }

    /// Sign in to one workspace, or start the step that the workspace needs first.
    async fn finish_workspace(
        &mut self,
        workspace: Workspace,
        options: Vec<(String, Workspace)>,
    ) -> Result<LoginStep> {
        if workspace.sso_required
            || (workspace.magic_login_url.is_empty() && workspace.magic_login_code.is_empty())
        {
            let step = browser_step(&workspace);
            self.state = State::Browser { workspace };
            return Ok(step);
        }
        if workspace.two_factor_required {
            return self
                .start_two_factor(
                    workspace,
                    options,
                    "Enter the authentication code required by this Slack workspace.",
                )
                .await;
        }
        match self.signin.login_workspace(&workspace).await {
            Ok((token, cookie)) => self.complete(&token, &cookie).await,
            Err(err) => {
                let text = format!(
                    "Could not sign in to {}: {}",
                    display_name(&workspace),
                    refusal_text(&err).unwrap_or("Slack did not complete the workspace login")
                );
                Ok(self.choose(options, &text))
            }
        }
    }

    async fn start_two_factor(
        &mut self,
        workspace: Workspace,
        options: Vec<(String, Workspace)>,
        instructions: &str,
    ) -> Result<LoginStep> {
        match self.signin.start_two_factor(&workspace).await {
            Ok(()) => {
                let step = two_factor_step(instructions);
                self.state = State::TwoFactor { workspace };
                Ok(step)
            }
            Err(err) => {
                let text = format!(
                    "Could not start two-factor authentication for {}: {}",
                    display_name(&workspace),
                    refusal_text(&err).unwrap_or("Slack did not complete the workspace login")
                );
                Ok(self.choose(options, &text))
            }
        }
    }

    async fn submit_two_factor(
        &mut self,
        workspace: Workspace,
        input: &LoginInput,
    ) -> Result<LoginStep> {
        let code = input
            .values
            .get("two_factor_code")
            .map(|value| value.trim().to_owned())
            .unwrap_or_default();
        if !(code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit())) {
            self.state = State::TwoFactor { workspace };
            return Ok(two_factor_step(
                "Enter the six-digit code from your authenticator app or SMS.",
            ));
        }
        match self.signin.submit_two_factor(&workspace, &code).await {
            Ok((token, cookie)) => self.complete(&token, &cookie).await,
            Err(SigninError::Refused { code: refusal, .. }) => match refusal.as_str() {
                "ratelimited" | "rate_limited" => {
                    self.state = State::TwoFactor { workspace };
                    Ok(two_factor_step(
                        "Slack is rate limiting authentication-code checks. Wait before trying again.",
                    ))
                }
                "two_factor_expired" | "two_factor_state_expired" => {
                    match self.signin.start_two_factor(&workspace).await {
                        Ok(()) => {
                            self.state = State::TwoFactor { workspace };
                            Ok(two_factor_step("That two-factor session expired. Enter a new authentication code."))
                        }
                        Err(_) => Ok(self.email("Could not start two-factor authentication. Enter your email to try again.")),
                    }
                }
                "invalid_2fa_code" | "invalid_two_factor_code" | "invalid_pin" => {
                    self.state = State::TwoFactor { workspace };
                    Ok(two_factor_step(
                        "Slack rejected that authentication code. Check the code and try again.",
                    ))
                }
                _ => {
                    self.state = State::TwoFactor { workspace };
                    Ok(two_factor_step(
                        "Slack did not complete two-factor authentication. Check the code and try again.",
                    ))
                }
            },
            Err(_) => {
                self.state = State::TwoFactor { workspace };
                Ok(two_factor_step(
                    "Slack did not complete two-factor authentication. Check the code and try again.",
                ))
            }
        }
    }

    async fn submit_browser(
        &mut self,
        workspace: Workspace,
        input: &LoginInput,
    ) -> Result<LoginStep> {
        if input.cookies.is_empty() {
            let step = browser_step(&workspace);
            self.state = State::Browser { workspace };
            return Ok(step);
        }
        let token = input.cookies.get("auth_token").cloned().unwrap_or_default();
        let cookie = input
            .cookies
            .get("cookie_token")
            .cloned()
            .unwrap_or_default();
        self.complete(&token, &cookie).await
    }

    /// Store the session. A session Slack made but the bridge cannot check starts the flow again.
    async fn complete(&mut self, token: &str, cookie: &str) -> Result<LoginStep> {
        match store_login(&self.ctx, &self.endpoints.api, token, Some(cookie)).await {
            Ok(login_id) => {
                self.done = true;
                Ok(LoginStep::Complete { login_id })
            }
            Err(_) => Ok(self.email(
                "Slack created a session, but the bridge could not validate it. Enter your email to try again.",
            )),
        }
    }
}

#[async_trait]
impl LoginProcess for EmailLogin {
    async fn next(&mut self, input: LoginInput) -> Result<LoginStep> {
        if self.done {
            return Err(Error::Protocol("the login already finished".to_owned()));
        }
        let state = std::mem::replace(&mut self.state, State::Email);
        match state {
            State::Start => {
                Ok(self.email("Enter the email address associated with your Slack account."))
            }
            State::Email => self.submit_email(&input).await,
            State::Captcha { email, site_key } => {
                self.submit_captcha(email, site_key, &input).await
            }
            State::Code { email } => self.submit_code(email, &input).await,
            State::Workspace { options } => self.submit_workspace(options, &input).await,
            State::Browser { workspace } => self.submit_browser(workspace, &input).await,
            State::TwoFactor { workspace } => self.submit_two_factor(workspace, &input).await,
        }
    }

    async fn cancel(&mut self) {
        let _ = self.ctx.delete_secret("token");
        let _ = self.ctx.delete_secret("cookies");
    }
}

/// The address step.
fn email_step(instructions: &str) -> LoginStep {
    LoginStep::UserInput {
        step_id: EMAIL_STEP.to_owned(),
        instructions: instructions.to_owned(),
        fields: vec![{
            let mut field = input_field("email", "Email", "email", "");
            field.pattern = r"^[^@\s]+@[^@\s]+\.[^@\s]+$".to_owned();
            field
        }],
    }
}

/// The CAPTCHA step: a web page on the sign-in host whose script returns the solved token.
fn captcha_step(site_key: &str, instructions: &str) -> LoginStep {
    LoginStep::Cookies {
        step_id: CAPTCHA_STEP.to_owned(),
        instructions: instructions.to_owned(),
        url: CAPTCHA_PAGE.to_owned(),
        user_agent: String::new(),
        fields: vec![CookieField {
            id: "captcha_token".to_owned(),
            required: true,
            sources: vec![CookieSource {
                kind: "javascript".to_owned(),
                name: "captcha_token".to_owned(),
                cookie_domain: String::new(),
            }],
        }],
        extract_js: captcha_js(site_key),
        wait_for_url_pattern: String::new(),
    }
}

/// The CAPTCHA script applied to its configuration, as the Go bridge builds it.
fn captcha_js(site_key: &str) -> String {
    format!("({CAPTCHA_JS})({})", json!({ "siteKey": site_key }))
}

fn code_step(instructions: &str) -> LoginStep {
    LoginStep::UserInput {
        step_id: CODE_STEP.to_owned(),
        instructions: instructions.to_owned(),
        fields: vec![{
            let mut field = input_field(
                "code",
                "Confirmation code",
                "text",
                "Six-character code from Slack",
            );
            field.pattern = r"^[a-zA-Z0-9]{3}-?[a-zA-Z0-9]{3}$".to_owned();
            field
        }],
    }
}

fn workspace_step(labels: Vec<String>, instructions: &str) -> LoginStep {
    LoginStep::UserInput {
        step_id: WORKSPACE_STEP.to_owned(),
        instructions: instructions.to_owned(),
        fields: vec![{
            let mut field = input_field(
                "workspace",
                "Workspace",
                "select",
                "Slack workspace to connect",
            );
            field.options = labels;
            field
        }],
    }
}

fn two_factor_step(instructions: &str) -> LoginStep {
    LoginStep::UserInput {
        step_id: TWO_FACTOR_STEP.to_owned(),
        instructions: instructions.to_owned(),
        fields: vec![{
            let mut field = input_field(
                "two_factor_code",
                "Authentication code",
                "text",
                "Six-digit code from your Slack authenticator or SMS",
            );
            field.pattern = r"^\d{6}$".to_owned();
            field
        }],
    }
}

/// The browser step of a workspace that cannot take an email login.
fn browser_step(workspace: &Workspace) -> LoginStep {
    let name = display_name(workspace);
    let instructions = if workspace.sso_required {
        format!(
            "{name} uses single sign-on. Sign in with your organization's account in the browser to continue."
        )
    } else {
        format!("{name} doesn't support email sign-in. Sign in to it in the browser to continue.")
    };
    let mut step = token_cookies_step(BROWSER_STEP, &sign_in_url(workspace), &instructions);
    if let LoginStep::Cookies { extract_js, .. } = &mut step {
        *extract_js = login::EXTRACT_TOKEN_JS.to_owned();
    }
    step
}

/// The page that signs the browser into a workspace.
fn sign_in_url(workspace: &Workspace) -> String {
    if let Ok(parsed) = reqwest::Url::parse(&workspace.url)
        && parsed.scheme() == "https"
        && parsed
            .host_str()
            .is_some_and(|host| host.ends_with(".slack.com"))
    {
        return parsed.to_string();
    }
    let domain = workspace
        .domain
        .strip_suffix(".slack.com")
        .unwrap_or(&workspace.domain);
    if !domain.is_empty() && !domain.contains(['.', '/', ':']) {
        return format!("https://{domain}.slack.com/");
    }
    CAPTCHA_PAGE.to_owned()
}

/// The name of a workspace as the user sees it.
fn display_name(workspace: &Workspace) -> String {
    if !workspace.name.is_empty() {
        workspace.name.clone()
    } else if !workspace.domain.is_empty() {
        workspace.domain.clone()
    } else {
        "Slack workspace".to_owned()
    }
}

/// The label a workspace is chosen by. Two workspaces with one label get `(2)`, `(3)`… appended.
fn label_workspaces(workspaces: Vec<Workspace>) -> Vec<(String, Workspace)> {
    let mut taken: BTreeMap<String, ()> = BTreeMap::new();
    let mut out = Vec::with_capacity(workspaces.len());
    for workspace in workspaces {
        let base = login_label(&workspace);
        let mut label = base.clone();
        let mut suffix = 2;
        while taken.contains_key(&label) {
            label = format!("{base} ({suffix})");
            suffix += 1;
        }
        taken.insert(label.clone(), ());
        out.push((label, workspace));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn login_label(workspace: &Workspace) -> String {
    let name = display_name(workspace);
    if !workspace.domain.is_empty() && workspace.domain != name {
        let domain = workspace
            .domain
            .strip_suffix(".slack.com")
            .unwrap_or(&workspace.domain);
        format!("{name} ({domain}.slack.com)")
    } else {
        name
    }
}

/// The text shown for a refused sign-in call; `None` for failures that are not refusals.
fn refusal_text(err: &SigninError) -> Option<&'static str> {
    match err {
        SigninError::Refused { code, .. } => Some(match code.as_str() {
            "ratelimited" | "rate_limited" | "code_request_limit_reached" => {
                "Slack is rate limiting email sign-in. Wait a few minutes before trying again."
            }
            "invalid_email" | "email_invalid" => {
                "Slack rejected that email address. Check it and try again."
            }
            "captcha_required" | "captcha_failed" => {
                "Slack requires a CAPTCHA for this sign-in. Complete the embedded challenge and try again."
            }
            _ => "Slack could not start email sign-in. Try again later.",
        }),
        _ => None,
    }
}

/// The bridge error of a failed sign-in call. Slack's codes are words, never credentials.
fn bridge_error(err: &SigninError) -> Error {
    match err {
        SigninError::Refused { method, code } => {
            Error::Protocol(format!("Slack {method} failed: {code}"))
        }
        SigninError::Http { method, status } if *status >= 500 => {
            Error::Network(format!("Slack {method} answered {status}"))
        }
        SigninError::Http { method, status } => {
            Error::Protocol(format!("Slack {method} answered {status}"))
        }
        SigninError::Transport(message) => Error::Network(message.clone()),
        SigninError::Malformed(what) => Error::Protocol(format!("Slack sent an unexpected {what}")),
    }
}

/// A plausible address: the pattern `^[^@\s]+@[^@\s]+\.[^@\s]+$`.
fn valid_email(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    let plain =
        |part: &str| !part.is_empty() && !part.chars().any(|c| c.is_whitespace() || c == '@');
    plain(local)
        && plain(domain)
        && domain
            .rsplit_once('.')
            .is_some_and(|(before, after)| !before.is_empty() && !after.is_empty())
}

/// A confirmation code: six letters or digits, optionally with a dash after the third.
fn valid_code(code: &str) -> bool {
    let compact: String = code.chars().filter(|c| *c != '-').collect();
    let shape = code.len() == 6 || (code.len() == 7 && code.as_bytes().get(3) == Some(&b'-'));
    shape && compact.len() == 6 && compact.chars().all(|c| c.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_and_codes_follow_the_go_patterns() {
        assert!(valid_email("ada@example.com"));
        assert!(!valid_email("ada@example"));
        assert!(!valid_email("ada @example.com"));
        assert!(!valid_email("@example.com"));
        assert!(valid_code("ABC-123"));
        assert!(valid_code("ABC123"));
        assert!(!valid_code("ABC12"));
        assert!(!valid_code("AB-C123"));
    }

    #[test]
    fn duplicate_workspace_labels_get_a_counter() {
        let make = |id: &str| Workspace {
            id: id.to_owned(),
            name: "Acme".to_owned(),
            ..Workspace::default()
        };
        let labels: Vec<String> = label_workspaces(vec![make("T1"), make("T2")])
            .into_iter()
            .map(|(label, _)| label)
            .collect();
        assert_eq!(labels, vec!["Acme".to_owned(), "Acme (2)".to_owned()]);
    }

    #[test]
    fn sign_in_urls_prefer_the_workspace_url_then_the_domain() {
        let url = Workspace {
            url: "https://acme.slack.com/".to_owned(),
            ..Workspace::default()
        };
        assert_eq!(sign_in_url(&url), "https://acme.slack.com/");
        let domain = Workspace {
            domain: "acme".to_owned(),
            ..Workspace::default()
        };
        assert_eq!(sign_in_url(&domain), "https://acme.slack.com/");
        assert_eq!(sign_in_url(&Workspace::default()), CAPTCHA_PAGE);
    }

    #[test]
    fn the_captcha_script_receives_its_site_key() {
        let js = captcha_js("KEY1");
        assert!(js.starts_with('('));
        assert!(js.ends_with(r#")({"siteKey":"KEY1"})"#));
    }

    #[test]
    fn refusals_have_the_go_instructions_and_other_failures_do_not() {
        let limited = SigninError::Refused {
            method: "signup.checkEmail",
            code: "ratelimited".to_owned(),
        };
        assert!(refusal_text(&limited).is_some_and(|text| text.contains("rate limiting")));
        assert!(refusal_text(&SigninError::Transport("x".to_owned())).is_none());
        assert!(matches!(bridge_error(&limited), Error::Protocol(_)));
    }
}
