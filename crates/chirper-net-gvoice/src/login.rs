//! The `cookies` login flow: the user signs in on voice.google.com and the page's cookies are
//! read and checked against the account record.
//!
//! Ported from `pkg/connector/login.go` of mautrix-gvoice (AGPL-3.0): the same step id, the same
//! cookie fields and their domains, the same required set, and the same check that the account
//! has a Google Voice number.

use std::collections::BTreeMap;

use async_trait::async_trait;

use chirper_bridge::model::{CookieField, CookieSource, LoginFlow, LoginStep};
use chirper_bridge::network::LoginInput;
use chirper_bridge::{Error, LoginCtx, LoginProcess, Result};

use crate::api::{Api, Hosts, USER_AGENT};
use crate::ids;
use crate::schema::BRANDING_GOOGLE_VOICE;
use crate::wire::Msg;

/// The id of the one login flow.
pub const FLOW_COOKIES: &str = "cookies";
/// The id of the cookie step the UI shows.
const STEP_COOKIES: &str = "fi.mau.gvoice.cookies";
/// The secret under which the cookies are stored, as a JSON object.
pub const COOKIES_SECRET: &str = "cookies";
/// The page the user signs in on.
const SIGN_IN_URL: &str = "https://voice.google.com/";

/// Every cookie the reference reads, with the domain it is set on.
const COOKIES: [(&str, &str); 8] = [
    ("OSID", "voice.google.com"),
    ("COMPASS", "clients6.google.com"),
    ("SID", ".google.com"),
    ("HSID", ".google.com"),
    ("SSID", ".google.com"),
    ("APISID", ".google.com"),
    ("SAPISID", ".google.com"),
    ("__Secure-1PSIDTS", ".google.com"),
];

/// The cookies without which the session cannot be used.
const REQUIRED: [&str; 5] = ["SID", "HSID", "SSID", "APISID", "SAPISID"];

/// The flow list the UI shows.
pub fn flows() -> Vec<LoginFlow> {
    vec![LoginFlow {
        id: FLOW_COOKIES.to_owned(),
        name: "Cookies".to_owned(),
        description: "Sign in to voice.google.com, then let Chirper read the cookies of that page."
            .to_owned(),
    }]
}

/// The step that asks the UI to read the cookies from the sign-in page.
pub fn cookie_step() -> LoginStep {
    let fields = COOKIES
        .iter()
        .map(|(name, domain)| CookieField {
            id: (*name).to_owned(),
            required: REQUIRED.contains(name),
            sources: vec![CookieSource {
                kind: "cookie".to_owned(),
                name: (*name).to_owned(),
                cookie_domain: (*domain).to_owned(),
            }],
        })
        .collect();
    LoginStep::Cookies {
        step_id: STEP_COOKIES.to_owned(),
        instructions:
            "Sign in to Google Voice in this window. Chirper reads the session cookies and keeps them in the system credential store."
                .to_owned(),
        url: SIGN_IN_URL.to_owned(),
        user_agent: USER_AGENT.to_owned(),
        fields,
        extract_js: String::new(),
        wait_for_url_pattern: String::new(),
    }
}

/// The cookie login: one cookie step, then the account is checked and stored.
pub struct CookieLogin {
    pub ctx: LoginCtx,
    pub hosts: Hosts,
    pub done: bool,
}

#[async_trait]
impl LoginProcess for CookieLogin {
    async fn next(&mut self, input: LoginInput) -> Result<LoginStep> {
        if self.done {
            return Err(Error::Protocol("the login already finished".to_owned()));
        }
        if input.cookies.is_empty() {
            return Ok(cookie_step());
        }
        let cookies = session_cookies(&input.cookies)?;
        let api = Api::new(self.ctx.http.clone(), self.hosts.clone(), &cookies)?;
        let account = api.get_account().await?;
        let e164 = primary_number(&account)?;
        let stored = serde_json::to_vec(&cookies).map_err(|err| Error::Internal(err.to_string()))?;
        self.ctx.set_secret(COOKIES_SECRET, &stored)?;
        self.done = true;
        Ok(LoginStep::Complete {
            login_id: ids::login_id(&e164),
        })
    }

    async fn cancel(&mut self) {
        let _ = self.ctx.delete_secret(COOKIES_SECRET);
    }
}

/// The known cookies from the submitted values, after the required ones are checked.
pub fn session_cookies(submitted: &BTreeMap<String, String>) -> Result<BTreeMap<String, String>> {
    for name in REQUIRED {
        if submitted.get(name).is_none_or(String::is_empty) {
            return Err(Error::Auth(format!(
                "the {name} cookie is missing; copy it from voice.google.com"
            )));
        }
    }
    Ok(COOKIES
        .iter()
        .filter_map(|(name, _)| {
            submitted
                .get(*name)
                .filter(|value| !value.is_empty())
                .map(|value| ((*name).to_owned(), value.clone()))
        })
        .collect())
}

/// The Google Voice number of an account: its primary number, or else its first Voice number.
pub fn primary_number(account: &Msg) -> Result<String> {
    let primary = account.string(1);
    if !primary.is_empty() {
        return Ok(primary);
    }
    account
        .messages(3)
        .map(|phone| phone.message(1))
        .find(|phone| phone.int(3) == BRANDING_GOOGLE_VOICE)
        .map(|phone| phone.message(1).string(1))
        .filter(|number| !number.is_empty())
        .ok_or_else(|| {
            Error::Auth(
                "this Google account has no Google Voice number; finish setting up Google Voice at voice.google.com"
                    .to_owned(),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::ACCOUNT;
    use crate::wire::decode_pblite;
    use serde_json::json;

    fn cookies(names: &[&str]) -> BTreeMap<String, String> {
        names
            .iter()
            .map(|name| ((*name).to_owned(), format!("{name}-value")))
            .collect()
    }

    #[test]
    fn the_step_lists_every_reference_cookie_with_its_domain() {
        let LoginStep::Cookies { step_id, fields, url, .. } = cookie_step() else {
            panic!("the cookie flow starts with a cookie step");
        };
        assert_eq!(step_id, "fi.mau.gvoice.cookies");
        assert_eq!(url, "https://voice.google.com/");
        assert_eq!(fields.len(), 8);
        let sapisid = fields.iter().find(|field| field.id == "SAPISID").expect("SAPISID");
        assert!(sapisid.required);
        assert_eq!(sapisid.sources[0].cookie_domain, ".google.com");
        let osid = fields.iter().find(|field| field.id == "OSID").expect("OSID");
        assert!(!osid.required);
        assert_eq!(osid.sources[0].cookie_domain, "voice.google.com");
    }

    #[test]
    fn a_missing_required_cookie_is_refused_by_name() {
        let error = session_cookies(&cookies(&["SID", "HSID", "SSID", "APISID"]))
            .expect_err("SAPISID is required");
        assert!(matches!(error, Error::Auth(message) if message.contains("SAPISID")));
    }

    #[test]
    fn only_the_reference_cookies_are_kept() {
        let mut submitted = cookies(&["SID", "HSID", "SSID", "APISID", "SAPISID", "OSID", "SPARE"]);
        submitted.insert("SAPISID".to_owned(), String::new());
        assert!(session_cookies(&submitted).is_err());
        submitted.insert("SAPISID".to_owned(), "x".to_owned());
        let kept = session_cookies(&submitted).expect("all required present");
        assert!(kept.contains_key("OSID"));
        assert!(!kept.contains_key("SPARE"));
    }

    /// A phone as the account record holds it: [accountPhone], accountPhone being
    /// [phoneNumber, defaultForOutbound, branding] and phoneNumber being [e164].
    fn phone(number: &str, branding: i64) -> serde_json::Value {
        json!([[[number], false, branding]])
    }

    #[test]
    fn the_primary_number_comes_first_then_the_voice_number() {
        let primary = decode_pblite(&ACCOUNT, &json!(["+15550001"])).expect("account");
        assert_eq!(primary_number(&primary).expect("number"), "+15550001");

        let with_voice = json!(["", null, [
            phone("+15559999", 2),
            phone("+15550002", BRANDING_GOOGLE_VOICE)
        ]]);
        let account = decode_pblite(&ACCOUNT, &with_voice).expect("account");
        assert_eq!(primary_number(&account).expect("number"), "+15550002");

        let without = json!(["", null, [phone("+15559999", 2)]]);
        let account = decode_pblite(&ACCOUNT, &without).expect("account");
        assert!(matches!(primary_number(&account), Err(Error::Auth(_))));
    }

    #[test]
    fn the_flow_list_has_the_one_reference_flow() {
        let flows = flows();
        assert_eq!(flows.len(), 1);
        assert_eq!(flows[0].id, FLOW_COOKIES);
    }
}
