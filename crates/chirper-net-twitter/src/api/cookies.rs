//! The cookie jar of one login. The jar holds the session secrets, so `Debug` never prints a value.

use std::collections::BTreeMap;
use std::fmt;

/// The cookie that names the signed-in user: `u=<id>` or its percent-encoded `u%3D<id>`.
const TWID: &str = "twid";
/// The session cookie.
pub const AUTH_TOKEN: &str = "auth_token";
/// The CSRF cookie, sent back as the `x-csrf-token` header.
pub const CT0: &str = "ct0";

/// Cookies by name.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Cookies {
    values: BTreeMap<String, String>,
}

impl fmt::Debug for Cookies {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cookies")
            .field("names", &self.values.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Cookies {
    /// A jar from stored name/value pairs. Empty names and values are dropped.
    pub fn from_map(map: BTreeMap<String, String>) -> Self {
        let values = map
            .into_iter()
            .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
            .filter(|(k, v)| !k.is_empty() && !v.is_empty())
            .collect();
        Self { values }
    }

    /// The stored pairs, for persisting.
    pub fn to_map(&self) -> BTreeMap<String, String> {
        self.values.clone()
    }

    /// The value of a cookie, or the empty string.
    pub fn get(&self, name: &str) -> &str {
        self.values.get(name).map_or("", String::as_str)
    }

    /// The `Cookie` header value.
    pub fn header(&self) -> String {
        self.values
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// The signed-in user id, read from the `twid` cookie.
    pub fn user_id(&self) -> Option<String> {
        let twid = self.get(TWID);
        let id = twid.replace("u%3D", "").replace("u=", "");
        let id = id.trim();
        (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())).then(|| id.to_owned())
    }

    /// Whether the two cookies every request needs are present.
    pub fn is_complete(&self) -> bool {
        !self.get(AUTH_TOKEN).is_empty() && !self.get(CT0).is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jar(pairs: &[(&str, &str)]) -> Cookies {
        Cookies::from_map(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        )
    }

    #[test]
    fn the_user_id_comes_from_twid_in_both_spellings() {
        assert_eq!(
            jar(&[("twid", "u=1234")]).user_id().as_deref(),
            Some("1234")
        );
        assert_eq!(jar(&[("twid", "u%3D99")]).user_id().as_deref(), Some("99"));
        assert_eq!(jar(&[("twid", "garbage")]).user_id(), None);
        assert_eq!(Cookies::default().user_id(), None);
    }

    #[test]
    fn a_complete_jar_has_both_auth_cookies() {
        assert!(!jar(&[(AUTH_TOKEN, "a")]).is_complete());
        assert!(jar(&[(AUTH_TOKEN, "a"), (CT0, "b")]).is_complete());
        assert!(!jar(&[(AUTH_TOKEN, "a"), (CT0, "  ")]).is_complete());
    }

    #[test]
    fn the_header_lists_every_cookie_and_debug_never_prints_a_value() {
        let cookies = jar(&[(AUTH_TOKEN, "secret-value"), (CT0, "c")]);
        assert_eq!(cookies.header(), "auth_token=secret-value; ct0=c");
        let shown = format!("{cookies:?}");
        assert!(shown.contains("auth_token"));
        assert!(!shown.contains("secret-value"));
    }
}
