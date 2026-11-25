//! The sign-in endpoints behind the email login: the calls a browser makes at `slack.com/signin`
//! and the magic-login exchange on `app.slack.com`.
//!
//! This is a port of `connector/login-email-api.go` (mautrix-slack, AGPL-3.0). The Go client keeps
//! a cookie jar; here the `d` cookie that Slack sets is remembered by hand, which is all the login
//! reads from it. Redirects are not followed: a 3xx answer is accepted as the Go code accepts it,
//! and its `Set-Cookie` is still read.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use reqwest::header::{COOKIE, HeaderMap, SET_COOKIE, USER_AGENT};
use serde::Deserialize;
use serde_json::Value;

use crate::wire::Workspace;

/// Largest answer read from a sign-in endpoint.
const MAX_RESPONSE_BYTES: usize = 16 << 20;
/// The user agent the sign-in pages send.
const USER_AGENT_VALUE: &str = "Chirper (https://chirper.app, 1.0)";

/// A failed sign-in call.
#[derive(Debug)]
pub enum SigninError {
    /// Slack answered `ok: false` (or 429) with an error code.
    Refused { method: &'static str, code: String },
    /// An HTTP status the login does not accept.
    Http { method: &'static str, status: u16 },
    /// The request did not complete.
    Transport(String),
    /// The answer is not what the login expects.
    Malformed(String),
}

pub type SigninResult<T> = Result<T, SigninError>;

/// The sign-in client. `root` is `https://slack.com` in production and `app` is
/// `https://app.slack.com`.
pub struct Signin {
    http: reqwest::Client,
    root: String,
    app: String,
    /// The raw `d` cookie Slack last set, as it arrived.
    cookie: Mutex<Option<String>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Checked {
    challenge_response: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SiteKey {
    sitekey: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct FoundWorkspaces {
    current_teams: Vec<WorkspaceGroup>,
    current_orgs: Vec<WorkspaceOrg>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WorkspaceGroup {
    teams: Vec<Workspace>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WorkspaceOrg {
    org: Option<Workspace>,
    teams: Vec<Workspace>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct BulkMagic {
    ok: bool,
    error: String,
    token_results: HashMap<String, MagicResult>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct MagicResult {
    error: String,
    redir: String,
    auth_redir: String,
    team: Option<Workspace>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AuthTeams {
    teams: HashMap<String, AuthTeam>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AuthTeam {
    token: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NativeLogin {
    token: String,
}

impl Signin {
    /// A client for the Slack root (`https://slack.com`) and the app root (`https://app.slack.com`).
    pub fn new(http: reqwest::Client, root: &str, app: &str) -> Self {
        Self {
            http,
            root: root.trim_end_matches('/').to_owned(),
            app: app.trim_end_matches('/').to_owned(),
            cookie: Mutex::new(None),
        }
    }

    /// `signup.checkEmail`: whether Slack wants a CAPTCHA before it sends a code.
    pub async fn check_email(&self, email: &str) -> SigninResult<bool> {
        let value = self
            .post(
                "signup.checkEmail",
                None,
                &[("email", email), ("get_info", "undefined")],
            )
            .await?;
        Ok(parse::<Checked>(value)?.challenge_response)
    }

    /// `auth.captcha`: the site key of the CAPTCHA the sign-in page must show.
    pub async fn captcha_site_key(&self) -> SigninResult<String> {
        let value = self
            .post("auth.captcha", Some("fetch-auth-captcha-signin"), &[])
            .await?;
        let key = parse::<SiteKey>(value)?.sitekey;
        if key.trim().is_empty() {
            return Err(SigninError::Malformed("no CAPTCHA site key".to_owned()));
        }
        Ok(key)
    }

    /// `signup.confirmEmail`: asks Slack to send the confirmation code, with the CAPTCHA answer if
    /// one was required.
    pub async fn confirm_email(&self, email: &str, captcha: Option<&str>) -> SigninResult<()> {
        let mut fields = vec![
            ("email", email),
            ("locale", "en-US"),
            ("entry_point", "signin"),
        ];
        if let Some(answer) = captcha {
            fields.push(("captcha_response", answer));
        }
        self.post("signup.confirmEmail", None, &fields)
            .await
            .map(|_| ())
    }

    /// `signin.confirmCode`: checks the confirmation code. The code has no dashes.
    pub async fn confirm_code(&self, email: &str, code: &str) -> SigninResult<()> {
        self.post(
            "signin.confirmCode",
            None,
            &[("email", email), ("code", code)],
        )
        .await
        .map(|_| ())
    }

    /// `signin.findWorkspaces`: every workspace the confirmed address can sign in to.
    pub async fn find_workspaces(&self) -> SigninResult<Vec<Workspace>> {
        let value = self
            .post("signin.findWorkspaces", Some("get_started_workspaces"), &[])
            .await?;
        let found: FoundWorkspaces = parse(value)?;
        let mut all: Vec<Workspace> = Vec::new();
        for group in found.current_teams {
            all.extend(group.teams);
        }
        for org in found.current_orgs {
            all.extend(org.org);
            all.extend(org.teams);
        }
        let mut seen: Vec<String> = Vec::new();
        all.retain(|workspace| {
            if workspace.id.is_empty() || seen.contains(&workspace.id) {
                return false;
            }
            seen.push(workspace.id.clone());
            true
        });
        Ok(all)
    }

    /// Obtain the browser token (`xoxc-…`) and cookie (`d`) of a workspace through the magic login
    /// links Slack returned for it.
    pub async fn login_workspace(&self, workspace: &Workspace) -> SigninResult<(String, String)> {
        let (body, client_url) = if workspace.magic_login_code.is_empty() {
            let body = self.get(&workspace.magic_login_url).await?;
            (body, self.client_url(&workspace.id))
        } else {
            self.magic_exchange(workspace).await?
        };
        let token = match find_auth_token(&body) {
            Some(token) => token,
            None => {
                let return_to = if client_url.is_empty() {
                    self.client_url(&workspace.id)
                } else {
                    client_url
                };
                self.fetch_auth_token(workspace, &return_to).await?
            }
        };
        let cookie = self
            .cookie_value()
            .ok_or_else(|| SigninError::Malformed("Slack set no cookie token".to_owned()))?;
        Ok((token, cookie))
    }

    /// Start the two-factor challenge of a workspace. Slack answering that a code is needed is the
    /// expected outcome.
    pub async fn start_two_factor(&self, workspace: &Workspace) -> SigninResult<()> {
        match self.native_login(&workspace.magic_login_code, None).await {
            Ok(_) => Ok(()),
            Err(err) if needs_two_factor_code(&err) => Ok(()),
            Err(err) => Err(err),
        }
    }

    /// Finish the two-factor challenge with the six-digit code.
    pub async fn submit_two_factor(
        &self,
        workspace: &Workspace,
        code: &str,
    ) -> SigninResult<(String, String)> {
        let token = self
            .native_login(&workspace.magic_login_code, Some(code))
            .await?;
        if token.is_empty() {
            return Err(SigninError::Malformed(
                "no token after two-factor".to_owned(),
            ));
        }
        let cookie = self
            .cookie_value()
            .ok_or_else(|| SigninError::Malformed("Slack set no cookie token".to_owned()))?;
        Ok((token, cookie))
    }

    /// The `d` cookie as Slack set it, percent-decoded.
    fn cookie_value(&self) -> Option<String> {
        let raw = self.cookie.lock().ok()?.clone()?;
        Some(percent_decode_str(&raw).decode_utf8_lossy().into_owned())
    }

    /// The URL of a workspace's client on the app host.
    fn client_url(&self, workspace_id: &str) -> String {
        if workspace_id.is_empty() {
            return String::new();
        }
        format!(
            "{}/client/{}",
            self.app,
            utf8_percent_encode(workspace_id, NON_ALPHANUMERIC)
        )
    }

    /// `auth.loginMagicBulk` on the app host. Returns the answer's text and the client URL it names.
    async fn magic_exchange(&self, workspace: &Workspace) -> SigninResult<(String, String)> {
        let url = format!(
            "{}/api/auth.loginMagicBulk?magic_tokens={}&ssb=1",
            self.app,
            utf8_percent_encode(&workspace.magic_login_code, NON_ALPHANUMERIC)
        );
        let body = self.get(&url).await?;
        let bulk: BulkMagic = serde_json::from_str(&body)
            .map_err(|_| SigninError::Malformed("magic login answer".to_owned()))?;
        if !bulk.ok {
            return Err(SigninError::Refused {
                method: "auth.loginMagicBulk",
                code: bulk.error,
            });
        }
        let result = bulk
            .token_results
            .get(&workspace.magic_login_code)
            .or_else(|| bulk.token_results.values().next())
            .ok_or_else(|| SigninError::Malformed("no magic login result".to_owned()))?;
        if !result.error.is_empty() {
            return Err(SigninError::Refused {
                method: "auth.loginMagicBulk",
                code: result.error.clone(),
            });
        }
        if !result.auth_redir.is_empty() {
            return Err(SigninError::Malformed(
                "the workspace needs a browser sign-in".to_owned(),
            ));
        }
        let client_url = result
            .team
            .as_ref()
            .map(|team| self.client_url(&team.id))
            .filter(|url| !url.is_empty())
            .or_else(|| Some(self.client_url(&workspace.id)).filter(|url| !url.is_empty()))
            .or_else(|| absolute_http(&result.redir))
            .unwrap_or_default();
        Ok((body, client_url))
    }

    /// `/auth?app=client` on the app host: the token of a workspace the browser session can open.
    async fn fetch_auth_token(
        &self,
        workspace: &Workspace,
        return_to: &str,
    ) -> SigninResult<String> {
        let path = reqwest::Url::parse(return_to)
            .map(|url| match url.query() {
                Some(query) => format!("{}?{query}", url.path()),
                None => url.path().to_owned(),
            })
            .unwrap_or_else(|_| return_to.to_owned());
        let url = format!(
            "{}/auth?app=client&response_type=json&return_to={}&teams={}",
            self.app,
            utf8_percent_encode(&path, NON_ALPHANUMERIC),
            utf8_percent_encode(&workspace.id, NON_ALPHANUMERIC)
        );
        let body = self.get(&url).await?;
        let teams: AuthTeams = serde_json::from_str(&body)
            .map_err(|_| SigninError::Malformed("client credentials".to_owned()))?;
        teams
            .teams
            .get(&workspace.id)
            .map(|team| team.token.clone())
            .filter(|token| !token.is_empty())
            .ok_or_else(|| SigninError::Malformed("no token for the workspace".to_owned()))
    }

    /// `auth.loginMagic`, with the two-factor code when there is one.
    async fn native_login(&self, magic_code: &str, pin: Option<&str>) -> SigninResult<String> {
        let mut fields = vec![
            ("magic_token", magic_code),
            ("two_factor_native_supported", "1"),
            ("two_factor_is_backup", "0"),
        ];
        if let Some(pin) = pin {
            fields.push(("two_factor_pin", pin));
        }
        let value = self.post("auth.loginMagic", None, &fields).await?;
        Ok(parse::<NativeLogin>(value)?.token)
    }

    /// POST a form to a sign-in method as a multipart body, the way the Go client does.
    async fn post(
        &self,
        method: &'static str,
        reason: Option<&str>,
        fields: &[(&str, &str)],
    ) -> SigninResult<Value> {
        let boundary = format!("----chirper{}", nonce());
        let mut body = String::new();
        for (name, value) in fields {
            body.push_str(&part(&boundary, name, value));
        }
        if let Some(reason) = reason {
            body.push_str(&part(&boundary, "_x_reason", reason));
        }
        body.push_str(&format!("--{boundary}--\r\n"));
        let url = format!("{}/api/{method}?_x_id=noversion-{}", self.root, nonce());
        let mut request = self
            .http
            .post(url)
            .header("Accept", "application/json")
            .header(
                "Content-Type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .header("Referer", format!("{}/signin", self.root))
            .header(USER_AGENT, USER_AGENT_VALUE)
            .body(body);
        if let Some(cookie) = self.cookie.lock().ok().and_then(|cookie| cookie.clone()) {
            request = request.header(COOKIE, format!("d={cookie}"));
        }
        let response = request
            .send()
            .await
            .map_err(|err| SigninError::Transport(err.to_string()))?;
        let status = response.status().as_u16();
        self.remember_cookie(response.headers());
        let bytes = response
            .bytes()
            .await
            .map_err(|err| SigninError::Transport(err.to_string()))?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(SigninError::Malformed("answer too large".to_owned()));
        }
        if status != 200 && status != 429 {
            return Err(SigninError::Http { method, status });
        }
        let value: Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) if status == 429 => {
                return Err(SigninError::Refused {
                    method,
                    code: "ratelimited".to_owned(),
                });
            }
            Err(_) => return Err(SigninError::Malformed(format!("{method} answer"))),
        };
        if status == 429 || value.get("ok").and_then(Value::as_bool) != Some(true) {
            let code = value
                .get("error")
                .and_then(Value::as_str)
                .filter(|code| !code.is_empty())
                .map_or_else(
                    || {
                        if status == 429 {
                            "ratelimited"
                        } else {
                            "unknown"
                        }
                        .to_owned()
                    },
                    str::to_owned,
                );
            return Err(SigninError::Refused { method, code });
        }
        Ok(value)
    }

    /// GET an app-host page; its text, when the status is one the login accepts.
    async fn get(&self, url: &str) -> SigninResult<String> {
        let response = self
            .http
            .get(url)
            .header(USER_AGENT, USER_AGENT_VALUE)
            .send()
            .await
            .map_err(|err| SigninError::Transport(err.to_string()))?;
        let status = response.status().as_u16();
        if !(200..400).contains(&status) {
            return Err(SigninError::Http {
                method: "magic login",
                status,
            });
        }
        self.remember_cookie(response.headers());
        let bytes = response
            .bytes()
            .await
            .map_err(|err| SigninError::Transport(err.to_string()))?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(SigninError::Malformed("answer too large".to_owned()));
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Keep the `d` cookie of any answer that sets one.
    fn remember_cookie(&self, headers: &HeaderMap) {
        for header in headers.get_all(SET_COOKIE) {
            let Ok(text) = header.to_str() else {
                continue;
            };
            let Some(rest) = text.strip_prefix("d=") else {
                continue;
            };
            let value = rest.split(';').next().unwrap_or_default();
            if !value.is_empty()
                && let Ok(mut cookie) = self.cookie.lock()
            {
                *cookie = Some(value.to_owned());
            }
        }
    }
}

/// The first browser token (`xoxc-` followed by letters, digits and dashes) in a text.
pub fn find_auth_token(text: &str) -> Option<String> {
    let mut rest = text;
    while let Some(index) = rest.find("xoxc-") {
        let candidate = &rest[index..];
        let end = candidate
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .unwrap_or(candidate.len());
        if end > "xoxc-".len() {
            return Some(candidate[..end].to_owned());
        }
        rest = &candidate["xoxc-".len()..];
    }
    None
}

fn needs_two_factor_code(err: &SigninError) -> bool {
    matches!(
        err,
        SigninError::Refused { code, .. }
            if matches!(
                code.as_str(),
                "two_factor_required"
                    | "missing_pin"
                    | "missing_pin_app"
                    | "missing_pin_app_sms"
                    | "missing_pin_sms"
                    | "missing_pin_sms_app"
                    | "missing_pin_sms_sms"
            )
    )
}

fn absolute_http(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    matches!(parsed.scheme(), "http" | "https").then(|| parsed.to_string())
}

fn parse<T: for<'de> Deserialize<'de>>(value: Value) -> SigninResult<T> {
    serde_json::from_value(value).map_err(|_| SigninError::Malformed("answer shape".to_owned()))
}

/// One multipart field.
fn part(boundary: &str, name: &str, value: &str) -> String {
    format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"\r\n\r\n{value}\r\n",
        name.replace('"', "%22")
    )
}

fn nonce() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_tokens_are_found_in_text() {
        let text = r#"{"token":"xoxc-123-abc_def","x":1}"#;
        assert_eq!(find_auth_token(text).as_deref(), Some("xoxc-123-abc"));
        assert_eq!(
            find_auth_token("xoxc- nothing xoxc-9"),
            Some("xoxc-9".to_owned())
        );
        assert_eq!(find_auth_token("no token here"), None);
    }

    #[test]
    fn two_factor_refusals_are_recognised() {
        let needed = SigninError::Refused {
            method: "auth.loginMagic",
            code: "missing_pin_sms".to_owned(),
        };
        assert!(needs_two_factor_code(&needed));
        let other = SigninError::Refused {
            method: "auth.loginMagic",
            code: "invalid_magic_token".to_owned(),
        };
        assert!(!needs_two_factor_code(&other));
    }

    #[test]
    fn multipart_parts_are_well_formed() {
        let text = part("B", "email", "a@b.c");
        assert_eq!(
            text,
            "--B\r\nContent-Disposition: form-data; name=\"email\"\r\n\r\na@b.c\r\n"
        );
    }

    #[test]
    fn a_set_cookie_is_remembered_raw_and_read_decoded() {
        let signin = Signin::new(
            reqwest::Client::new(),
            "https://slack.com",
            "https://app.slack.com",
        );
        let mut headers = HeaderMap::new();
        headers.append(
            SET_COOKIE,
            "d=xoxd-a%2Fb%3D; Path=/; Secure".parse().expect("header"),
        );
        signin.remember_cookie(&headers);
        assert_eq!(signin.cookie_value().as_deref(), Some("xoxd-a/b="));
    }
}
