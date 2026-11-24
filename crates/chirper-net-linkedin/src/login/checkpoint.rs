//! The verification checkpoint: the page LinkedIn shows after the password when it wants a code
//! from e-mail, SMS, an authenticator app, or an approval in the LinkedIn app.
//!
//! Only the form the reference recognises is submitted: its action, its method, its hidden
//! fields and its six-digit field are checked, and a code is never sent again after a rejection.
//! Reference: `linkedingo/login_checkpoint.go`.

use std::fmt::Write as _;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::Method;
use reqwest::header::HeaderMap;
use serde_json::Value;

use super::client::{Attached, Authenticated, Client, Failure, Kind, classify, request_uri};
use super::html::{self, Token};
use super::sdui::{self, AUTH_REQUEST};
use crate::api;

/// The form ids the checkpoint pages use.
const FORM_EMAIL: &str = "email-pin-challenge";
const FORM_SMS: &str = "two-step-challenge";
const FORM_APP: &str = "in-app-challenge";
/// The recovery-code form that marks the authenticator-app page.
const FORM_RECOVERY: &str = "two-step-challenge-recovery-code";
/// The action of each form, as the reference accepts it.
const ACTION_CODE: &str = "/checkpoint/challenge/verify";
const ACTION_APP: &str = "/checkpoint/challenge/verifyV2";
const ACTION_RECOVERY: &str = "/checkpoint/challenge/verifyCode";
/// The app page's flavour, and the longest polling window it may ask for.
const APP_FLAVOUR: &str = "\"CONSUMER_LOGIN\"";
const MAX_POLL_SECONDS: i64 = 86_400;
/// The app states that mean the request is still waiting.
const APP_WAITING: [&str; 2] = ["CREATED", "SHOWN"];
/// The app state that means the user approved.
const APP_SOLVED: &str = "SOLVED";
/// The redirect hops a checkpoint load follows.
const MAX_REDIRECTS: usize = 5;

/// The kind of verification a checkpoint asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FormKind {
    /// A code sent to the account's e-mail.
    Email,
    /// A code sent by SMS.
    Sms,
    /// A code from an authenticator app.
    Totp,
    /// An approval in the LinkedIn app.
    App,
}

/// The form of a checkpoint page.
struct Form {
    kind: FormKind,
    action: String,
    /// The hidden fields the form carries, in document order.
    fields: Vec<(String, String)>,
    poll_interval: Duration,
    /// When the approval request expires, in milliseconds since the epoch.
    expires_at_ms: i64,
    try_another_way: bool,
}

/// A checkpoint: where the login stopped, and the form to answer.
pub(super) struct Checkpoint {
    client: Client,
    /// The request target of the page, relative to the origin.
    path: String,
    form: Option<Form>,
    /// The page as last loaded, for the completion check.
    page: Vec<u8>,
    /// The headers of the last page, which the completion request needs.
    headers: HeaderMap,
}

impl Checkpoint {
    /// A checkpoint at `path`, not loaded yet.
    pub(super) fn new(client: Client, path: String) -> Self {
        Self {
            client,
            path,
            form: None,
            page: Vec::new(),
            headers: HeaderMap::new(),
        }
    }

    /// The kind of verification the page asks for, when its form is one this driver knows.
    pub(super) fn kind(&self) -> Option<FormKind> {
        self.form.as_ref().map(|form| form.kind)
    }

    /// Whether the app page offers another verification method.
    pub(super) fn can_try_another_way(&self) -> bool {
        self.form
            .as_ref()
            .is_some_and(|form| form.kind == FormKind::App && form.try_another_way)
    }

    /// Loads the checkpoint page, following same-origin redirects. A page that turns out to be
    /// the feed leaves no form, and the caller finishes the login from the cookies.
    pub(super) async fn load(&mut self) -> Result<(), Failure> {
        self.page.clear();
        self.form = None;
        for _ in 0..MAX_REDIRECTS {
            let url = format!("{}{}", self.client.base(), self.path);
            let reply = self
                .client
                .exchange(
                    "checkpoint",
                    Method::GET,
                    &url,
                    vec![
                        ("Accept", "text/html,application/xhtml+xml".to_owned()),
                        ("Referer", format!("{}/login", self.client.base())),
                        ("Sec-Fetch-Dest", "document".to_owned()),
                        ("Sec-Fetch-Mode", "navigate".to_owned()),
                        ("Sec-Fetch-Site", "same-origin".to_owned()),
                    ],
                    None,
                )
                .await?;
            if reply.is_redirect() {
                let location = reply
                    .location()
                    .and_then(|value| self.client.resolve(&url, value))
                    .ok_or_else(|| Failure::new(Kind::Blocked, "checkpoint", reply.status))?;
                self.path = request_uri(&location);
                continue;
            }
            if let Some(failure) = classify("checkpoint", &reply) {
                return Err(failure);
            }
            if reply.status != 200 {
                return Err(Failure::new(Kind::BadResponse, "checkpoint", reply.status));
            }
            if is_feed(&self.path) {
                return Ok(());
            }
            self.form = parse_form(&reply.body);
            self.page = reply.body;
            self.headers = reply.headers;
            return Ok(());
        }
        Err(Failure::new(Kind::Blocked, "checkpoint", 0))
    }

    /// Submits the six-digit code of an e-mail, SMS or authenticator-app checkpoint.
    pub(super) async fn submit_code(&mut self, code: &str) -> Result<Authenticated, Failure> {
        let kind = match self.kind() {
            Some(kind @ (FormKind::Email | FormKind::Sms | FormKind::Totp)) => kind,
            _ => return Err(Failure::new(Kind::Unsupported, "verify_code", 0)),
        };
        if !valid_code(code) {
            return Err(self.current(Kind::Rejected, code_stage(kind), 0));
        }
        let mut fields = self.fields();
        fields.push(("pin".to_owned(), code.to_owned()));
        self.submit_form(kind, fields).await
    }

    /// Asks the app checkpoint for another verification method.
    pub(super) async fn try_another_way(&mut self) -> Result<Authenticated, Failure> {
        if !self.can_try_another_way() {
            return Err(Failure::new(Kind::Unsupported, "app_alternative", 0));
        }
        let mut fields = self.fields();
        set_field(&mut fields, "userResponse", "TRY_ANOTHER_WAY");
        self.submit_form(FormKind::App, fields).await
    }

    /// Polls the app request until the user approves it, it expires, or it fails. Only a
    /// `SOLVED` state authorises submitting the original form.
    pub(super) async fn wait_for_app(&mut self) -> Result<Authenticated, Failure> {
        let (mut fields, interval, expires_at_ms, challenge_id) = match self.form.as_ref() {
            Some(form) if form.kind == FormKind::App => (
                form.fields.clone(),
                form.poll_interval,
                form.expires_at_ms,
                field(&form.fields, "challengeId")
                    .unwrap_or_default()
                    .to_owned(),
            ),
            _ => return Err(Failure::new(Kind::Unsupported, "app_poll", 0)),
        };
        set_field(&mut fields, "recognizedDevice", "true");
        set_field(&mut fields, "userResponse", "");
        loop {
            let (boundary, body) = multipart(&fields);
            let url = format!(
                "{}/checkpoint/challengesV2/poll/{}",
                self.client.base(),
                api::path_escape(&challenge_id)
            );
            let headers = vec![
                ("Accept", "*/*".to_owned()),
                (
                    "Content-Type",
                    format!("multipart/form-data; boundary={boundary}"),
                ),
                ("Origin", self.client.base().to_owned()),
                ("Referer", format!("{}{}", self.client.base(), self.path)),
                ("X-Requested-With", "XMLHttpRequest".to_owned()),
                ("Sec-Fetch-Dest", "empty".to_owned()),
                ("Sec-Fetch-Mode", "cors".to_owned()),
                ("Sec-Fetch-Site", "same-origin".to_owned()),
            ];
            let reply = self
                .client
                .exchange("app_poll", Method::POST, &url, headers, Some(body))
                .await?;
            if let Some(failure) = classify("app_poll", &reply) {
                return Err(failure);
            }
            let now = now_ms();
            if now >= expires_at_ms {
                return Err(Failure::new(
                    Kind::Rejected,
                    "app_approval_expired",
                    reply.status,
                ));
            }
            let state = serde_json::from_slice::<Value>(&reply.body)
                .ok()
                .and_then(|value| {
                    value
                        .get("pollingResponseChallengeStateV2")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                });
            let state = match state {
                Some(state) if reply.status == 200 => state,
                _ => return Err(Failure::new(Kind::BadResponse, "app_poll", reply.status)),
            };
            if state == APP_SOLVED {
                return self.submit_form(FormKind::App, fields).await;
            }
            if !APP_WAITING.contains(&state.as_str()) {
                return Err(Failure::new(Kind::Challenge, "app_poll", reply.status));
            }
            let remaining = u64::try_from(expires_at_ms - now).unwrap_or(0);
            tokio::time::sleep(interval.min(Duration::from_millis(remaining))).await;
        }
    }

    /// Posts a form of the checkpoint and follows the answer: the feed, the next checkpoint, or
    /// the automatic completion request.
    async fn submit_form(
        &mut self,
        kind: FormKind,
        fields: Vec<(String, String)>,
    ) -> Result<Authenticated, Failure> {
        let action = self
            .form
            .as_ref()
            .map(|form| form.action.clone())
            .unwrap_or_default();
        let stage = verify_stage(kind);
        let url = format!("{}{action}", self.client.base());
        let headers = vec![
            ("Accept", "text/html,application/xhtml+xml".to_owned()),
            (
                "Content-Type",
                "application/x-www-form-urlencoded".to_owned(),
            ),
            ("Origin", self.client.base().to_owned()),
            ("Referer", format!("{}{}", self.client.base(), self.path)),
            ("Sec-Fetch-Dest", "document".to_owned()),
            ("Sec-Fetch-Mode", "navigate".to_owned()),
            ("Sec-Fetch-Site", "same-origin".to_owned()),
        ];
        let reply = self
            .client
            .exchange(
                stage,
                Method::POST,
                &url,
                headers,
                Some(encode_form(&fields).into_bytes()),
            )
            .await?;
        if let Some(failure) = classify(stage, &reply) {
            if failure.kind != Kind::Rejected {
                return Err(failure);
            }
            // A rejected code comes back as the same form, which is kept for the retry.
            self.path = action;
            self.form = parse_form(&reply.body);
            self.page = reply.body;
            self.headers = reply.headers;
            return Err(self.challenge_result(kind, stage, failure.status));
        }
        let status = reply.status;
        match status {
            301..=303 => {
                let location = reply
                    .location()
                    .and_then(|value| self.client.resolve(&url, value))
                    .ok_or_else(|| Failure::new(Kind::Blocked, stage, status))?;
                self.path = request_uri(&location);
                self.load().await?;
                if is_feed(&self.path) && self.client.has_session() {
                    return Ok(self.client.authenticated());
                }
            }
            200 => {
                self.form = parse_form(&reply.body);
                self.page = reply.body;
                self.headers = reply.headers;
                self.path = action;
            }
            // A code or an approval is never replayed on a 307 or 308 redirect.
            _ => return Err(Failure::new(Kind::Challenge, stage, status)),
        }
        if self.form.is_some() {
            return Err(self.challenge_result(kind, stage, status));
        }
        match sdui::parse_completion(&self.page) {
            Some(completion) => {
                let body =
                    sdui::marshal_action(&completion.action, Vec::new(), &completion.screen_id);
                self.client
                    .submit(
                        AUTH_REQUEST,
                        body,
                        &self.headers,
                        "checkpoint_complete",
                        &self.path,
                    )
                    .await
            }
            None => Err(Failure::new(Kind::Challenge, stage, status)),
        }
    }

    /// The failure for a form that came back: a rejected code when the same kind of form is
    /// shown again, otherwise a challenge. Either keeps this checkpoint for the next step.
    fn challenge_result(&self, previous: FormKind, stage: &'static str, status: u16) -> Failure {
        if previous != FormKind::App && self.kind() == Some(previous) {
            return self.current(Kind::Rejected, code_stage(previous), status);
        }
        self.current(Kind::Challenge, stage, status)
    }

    /// A failure that belongs to this checkpoint.
    fn current(&self, kind: Kind, stage: &'static str, status: u16) -> Failure {
        let mut failure = Failure::new(kind, stage, status);
        failure.attached = Attached::Current;
        failure
    }

    /// The hidden fields of the current form.
    fn fields(&self) -> Vec<(String, String)> {
        self.form
            .as_ref()
            .map(|form| form.fields.clone())
            .unwrap_or_default()
    }
}

/// Whether a request target is the feed.
fn is_feed(path: &str) -> bool {
    matches!(path.split('?').next(), Some("/feed" | "/feed/"))
}

/// The stage a rejected code is reported under.
fn code_stage(kind: FormKind) -> &'static str {
    match kind {
        FormKind::Email => "email_code",
        FormKind::Sms => "sms_code",
        FormKind::Totp => "totp_code",
        FormKind::App => "app_code",
    }
}

/// The stage a form submission is reported under.
fn verify_stage(kind: FormKind) -> &'static str {
    match kind {
        FormKind::Email => "verify_email",
        FormKind::Sms => "verify_sms",
        FormKind::Totp => "verify_totp",
        FormKind::App => "verify_app",
    }
}

/// Six ASCII digits.
fn valid_code(code: &str) -> bool {
    code.len() == 6 && code.bytes().all(|byte| byte.is_ascii_digit())
}

/// The value of a field, first occurrence.
fn field<'a>(fields: &'a [(String, String)], name: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// Replaces every value of a field with one value, as `url.Values.Set` does.
fn set_field(fields: &mut Vec<(String, String)>, name: &str, value: &str) {
    fields.retain(|(key, _)| key != name);
    fields.push((name.to_owned(), value.to_owned()));
}

/// The form body: keys sorted, values escaped as Go's `url.Values.Encode` escapes them.
fn encode_form(fields: &[(String, String)]) -> String {
    let mut sorted: Vec<&(String, String)> = fields.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    sorted
        .iter()
        .map(|(key, value)| format!("{}={}", api::query_escape(key), api::query_escape(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// A multipart body of text fields, with its boundary.
fn multipart(fields: &[(String, String)]) -> (String, Vec<u8>) {
    let boundary = api::uuid_v4();
    let mut body = String::new();
    for (name, value) in fields {
        let name = name.replace('\\', "\\\\").replace('"', "\\\"");
        let _ = write!(
            body,
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        );
    }
    let _ = write!(body, "--{boundary}--\r\n");
    (boundary, body.into_bytes())
}

/// The current time in milliseconds since the epoch.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// Reads the checkpoint form of a page. `None` unless the page has exactly the form shape the
/// reference recognises: a POST to its own action, a hidden csrf token and challenge id, and the
/// six-digit field for the kind of code it asks for.
fn parse_form(body: &[u8]) -> Option<Form> {
    let page = String::from_utf8_lossy(body);
    let tokens = html::tokens(&page);
    let mut codes: Vec<(String, String)> = Vec::new();
    let mut try_another_way = false;
    let mut has_recovery = false;
    let mut chosen = None;
    for (index, token) in tokens.iter().enumerate() {
        let Token::Start { name, text, .. } = token else {
            continue;
        };
        let id = token.attr("id").unwrap_or_default();
        if name == "form" {
            if matches!(id, FORM_EMAIL | FORM_SMS | FORM_APP) {
                chosen = Some(index);
            } else if id == FORM_RECOVERY && token.attr("action") == Some(ACTION_RECOVERY) {
                has_recovery = true;
            }
        } else if name == "code" && !text.is_empty() {
            codes.push((id.to_owned(), text.trim().to_owned()));
        }
        if id == "try-another-way" {
            try_another_way = true;
        }
    }
    let code = |wanted: &str| {
        codes
            .iter()
            .find(|(id, _)| id == wanted)
            .map(|(_, value)| value.as_str())
    };
    let form_token = &tokens[chosen?];
    if !form_token
        .attr("method")
        .is_some_and(|method| method.eq_ignore_ascii_case("post"))
    {
        return None;
    }
    let kind = match form_token.attr("id")? {
        FORM_EMAIL => FormKind::Email,
        FORM_SMS if has_recovery => FormKind::Totp,
        FORM_SMS => FormKind::Sms,
        FORM_APP => FormKind::App,
        _ => return None,
    };
    let action = form_token.attr("action").unwrap_or_default().to_owned();
    let mut form = Form {
        kind,
        action: action.clone(),
        fields: Vec::new(),
        poll_interval: Duration::ZERO,
        expires_at_ms: 0,
        try_another_way,
    };
    if kind == FormKind::App {
        if action != ACTION_APP || code("flavour") != Some(APP_FLAVOUR) {
            return None;
        }
        let number = |wanted: &str| {
            code(wanted)
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(0)
        };
        let start = number("startTimestampInMillis");
        let ttl = number("totalPollingTimeInSeconds");
        let interval = number("pollingFrequencyInSeconds");
        if start <= 0 || ttl <= 0 || ttl > MAX_POLL_SECONDS || interval <= 0 || interval > ttl {
            return None;
        }
        form.expires_at_ms = start.saturating_add(ttl.saturating_mul(1000));
        form.poll_interval = Duration::from_secs(u64::try_from(interval).ok()?);
    } else if action != ACTION_CODE {
        return None;
    }
    let mut has_pin = false;
    for token in &tokens[chosen? + 1..] {
        if matches!(token, Token::End { name } if name == "form") {
            break;
        }
        if token.name() != "input" {
            continue;
        }
        let field_name = token.attr("name").unwrap_or_default();
        if field_name == "pin" && token.attr("maxlength") == Some("6") {
            has_pin = (kind == FormKind::Email
                && token.attr("autocomplete") == Some("one-time-code"))
                || (matches!(kind, FormKind::Sms | FormKind::Totp)
                    && token.attr("id") == Some("input__phone_verification_pin")
                    && token.attr("type") == Some("tel"));
        } else if !field_name.is_empty()
            && field_name != "pin"
            && token.attr("type") == Some("hidden")
        {
            form.fields.push((
                field_name.to_owned(),
                token.attr("value").unwrap_or_default().to_owned(),
            ));
        } else if field_name == "recognizedDevice"
            && token.attr("type") == Some("checkbox")
            && token.has_attr("checked")
        {
            let value = token.attr("value").filter(|value| !value.is_empty());
            form.fields
                .push((field_name.to_owned(), value.unwrap_or("on").to_owned()));
        }
    }
    let has = |name: &str| field(&form.fields, name).is_some_and(|value| !value.is_empty());
    if !has("csrfToken") || !has("challengeId") {
        return None;
    }
    if kind != FormKind::App && !has_pin {
        return None;
    }
    if kind == FormKind::App && field(&form.fields, "userResponse").is_none() {
        return None;
    }
    Some(form)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A checkpoint page with one form of the given id and PIN input.
    fn page_with(form_id: &str, csrf: &str, pin_input: &str) -> String {
        format!(
            r#"<html><form id="unrelated"><input type="hidden" name="unrelated" value="ignore"></form>
<form id="{form_id}" method="post" action="/checkpoint/challenge/verify">
<input type="hidden" name="csrfToken" value="{csrf}">
<input type="hidden" name="challengeId" value="challenge-sentinel">
<input type="hidden" name="challengeData" value="opaque&amp;value=preserved">
{pin_input}
</form></html>"#
        )
    }

    fn email_page(csrf: &str) -> String {
        page_with(
            FORM_EMAIL,
            csrf,
            r#"<input name="pin" type="number" maxlength="6" autocomplete="one-time-code">"#,
        )
    }

    #[test]
    fn only_the_observed_email_form_is_accepted() {
        let valid = email_page("csrf-sentinel");
        let form = parse_form(valid.as_bytes()).expect("the observed form parses");
        assert_eq!(form.kind, FormKind::Email);
        assert_eq!(
            field(&form.fields, "challengeData"),
            Some("opaque&value=preserved")
        );
        assert!(field(&form.fields, "unrelated").is_none());
        for (from, to) in [
            ("email-pin-challenge", "other-challenge"),
            ("method=\"post\"", "method=\"get\""),
            (
                "/checkpoint/challenge/verify",
                "https://attacker.invalid/verify",
            ),
            ("one-time-code", "password"),
            ("csrf-sentinel", ""),
            ("challenge-sentinel", ""),
        ] {
            let changed = valid.replace(from, to);
            assert!(parse_form(changed.as_bytes()).is_none(), "accepted {from}");
        }
    }

    #[test]
    fn the_authenticator_page_is_totp_and_the_sms_page_is_sms() {
        let sms = page_with(
            FORM_SMS,
            "c",
            r#"<input name="pin" maxlength="6" id="input__phone_verification_pin" type="tel">"#,
        );
        assert_eq!(
            parse_form(sms.as_bytes()).map(|f| f.kind),
            Some(FormKind::Sms)
        );
        let totp = format!(
            "{sms}<form id=\"two-step-challenge-recovery-code\" action=\"/checkpoint/challenge/verifyCode\"></form>"
        );
        assert_eq!(
            parse_form(totp.as_bytes()).map(|f| f.kind),
            Some(FormKind::Totp)
        );
    }

    /// The in-app approval page, with its polling codes.
    fn app_page(flavour: &str, ttl: &str, interval: &str) -> String {
        format!(
            r#"<html><form id="in-app-challenge" method="post" action="/checkpoint/challenge/verifyV2">
<code id="flavour">{flavour}</code>
<code id="startTimestampInMillis">1700000000000</code>
<code id="totalPollingTimeInSeconds">{ttl}</code>
<code id="pollingFrequencyInSeconds">{interval}</code>
<input type="hidden" name="csrfToken" value="c">
<input type="hidden" name="challengeId" value="ch">
<input type="hidden" name="userResponse" value="">
</form><span id="try-another-way">other</span></html>"#
        )
    }

    #[test]
    fn the_app_page_needs_its_flavour_and_a_sane_polling_window() {
        let flavour = "\"CONSUMER_LOGIN\"";
        let form = parse_form(app_page(flavour, "300", "2").as_bytes()).expect("parses");
        assert_eq!(form.kind, FormKind::App);
        assert!(form.try_another_way);
        assert_eq!(form.poll_interval, Duration::from_secs(2));
        assert_eq!(form.expires_at_ms, 1_700_000_000_000 + 300_000);
        assert!(parse_form(app_page("\"OTHER\"", "300", "2").as_bytes()).is_none());
        assert!(parse_form(app_page(flavour, "0", "2").as_bytes()).is_none());
        assert!(parse_form(app_page(flavour, "300", "301").as_bytes()).is_none());
        assert!(parse_form(app_page(flavour, "86401", "2").as_bytes()).is_none());
    }

    #[test]
    fn form_bodies_are_sorted_and_escaped() {
        let fields = vec![
            ("pin".to_owned(), "012345".to_owned()),
            ("challengeData".to_owned(), "a&b c".to_owned()),
        ];
        assert_eq!(encode_form(&fields), "challengeData=a%26b+c&pin=012345");
    }

    #[test]
    fn codes_are_six_digits() {
        assert!(valid_code("012345"));
        assert!(!valid_code("01234"));
        assert!(!valid_code("01234a"));
        assert!(!valid_code(" 012345"));
    }

    #[test]
    fn only_the_feed_counts_as_the_feed() {
        assert!(is_feed("/feed/"));
        assert!(is_feed("/feed?x=1"));
        assert!(!is_feed("/feed/more"));
        assert!(!is_feed("/checkpoint/feed"));
    }
}
