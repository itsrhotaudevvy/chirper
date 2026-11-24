//! The credentials of one paired Google Messages session: the phone and browser identities, the
//! AES/HMAC keys shared with the phone, the refresh key, the short-lived Tachyon token and, for a
//! Google account, the web cookies. Stored as one JSON secret (`session`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::crypto::{RefreshKey, RequestCrypto};
use crate::proto::pairing::Device;

/// Everything the driver needs to talk to a paired phone again.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AuthData {
    /// The keys shared with the phone; absent until pairing finished.
    #[serde(default)]
    pub request_crypto: Option<RequestCrypto>,
    /// Signs the token refresh requests.
    #[serde(default)]
    pub refresh_key: Option<RefreshKey>,
    /// This browser, as the server knows it.
    #[serde(default)]
    pub browser: Option<Device>,
    /// The paired phone (for a Google account, the account email, lower-cased).
    #[serde(default)]
    pub mobile: Option<Device>,
    /// The Tachyon auth token of every request; empty until the first refresh.
    #[serde(default)]
    pub tachyon_token: Vec<u8>,
    /// Unix milliseconds at which the token stops being valid.
    #[serde(default)]
    pub tachyon_expiry_ms: i64,
    /// Lifetime of the token in microseconds, as the server reported it.
    #[serde(default)]
    pub tachyon_ttl_us: i64,
    /// The session id of the web client, 32 hex digits (the `deviceID` of the config).
    #[serde(default)]
    pub session_id: String,
    /// The registration id of the Google account's destination device, when paired through a Google account.
    #[serde(default)]
    pub dest_reg_id: String,
    /// The phone's "unknown timestamp" id, which identifies it inside a Google account.
    #[serde(default)]
    pub dest_phone_id: u64,
    /// The id of the Gaia pairing attempt that produced the keys.
    #[serde(default)]
    pub pairing_id: String,
    /// Google web cookies, sent with every request of a Google account.
    #[serde(default)]
    pub cookies: BTreeMap<String, String>,
}

impl AuthData {
    /// Whether this is a Google account session (it talks to the Google host and uses cookies).
    pub fn is_google_account(&self) -> bool {
        !self.dest_reg_id.is_empty()
    }

    /// Whether any cookies are stored.
    pub fn has_cookies(&self) -> bool {
        !self.cookies.is_empty()
    }

    /// The network name the Google account uses in auth messages.
    pub fn auth_network(&self) -> &'static str {
        if self.is_google_account() {
            "GDitto"
        } else {
            ""
        }
    }

    /// Whether the phone pairing is complete enough to message with.
    pub fn is_paired(&self) -> bool {
        self.browser.is_some() && self.request_crypto.is_some()
    }

    /// Store a fresh token and its lifetime. A zero TTL means 24 hours, as the Go client assumes.
    pub fn set_token(&mut self, token: Vec<u8>, ttl_us: i64, now_ms: i64) {
        let ttl_us = if ttl_us == 0 {
            24 * 60 * 60 * 1_000_000
        } else {
            ttl_us
        };
        self.tachyon_token = token;
        self.tachyon_ttl_us = ttl_us;
        self.tachyon_expiry_ms = now_ms + ttl_us / 1000;
    }

    /// The token is missing or will expire within `buffer_ms`.
    pub fn token_needs_refresh(&self, now_ms: i64, buffer_ms: i64) -> bool {
        self.tachyon_token.is_empty() || self.tachyon_expiry_ms - now_ms <= buffer_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_without_ttl_lasts_a_day() {
        let mut auth = AuthData::default();
        auth.set_token(vec![1], 0, 1_000);
        assert_eq!(auth.tachyon_expiry_ms, 1_000 + 86_400_000);
    }

    #[test]
    fn a_token_is_refreshed_inside_the_buffer() {
        let mut auth = AuthData::default();
        auth.set_token(vec![1], 3_600_000_000, 0);
        assert!(!auth.token_needs_refresh(0, 60_000));
        assert!(auth.token_needs_refresh(3_560_000, 60_000));
        assert!(AuthData::default().token_needs_refresh(0, 0));
    }

    #[test]
    fn a_google_session_is_recognised_by_its_destination_device() {
        let mut auth = AuthData::default();
        assert!(!auth.is_google_account());
        auth.dest_reg_id = "uuid".to_owned();
        assert!(auth.is_google_account());
        assert_eq!(auth.auth_network(), "GDitto");
    }

    #[test]
    fn the_session_survives_a_json_round_trip() {
        let mut auth = AuthData::default();
        auth.request_crypto = Some(RequestCrypto::random().expect("keys"));
        auth.refresh_key = Some(RefreshKey::generate().expect("key"));
        auth.browser = Some(Device {
            user_id: 7,
            source_id: "browser".to_owned(),
            network: "Bugle".to_owned(),
        });
        auth.cookies.insert("SID".to_owned(), "abc".to_owned());
        let json = serde_json::to_string(&auth).expect("serialises");
        let back: AuthData = serde_json::from_str(&json).expect("parses");
        assert!(back.is_paired());
        assert_eq!(back.cookies.get("SID").map(String::as_str), Some("abc"));
        assert_eq!(
            back.refresh_key.expect("key").pkcs8(),
            auth.refresh_key.expect("key").pkcs8()
        );
    }
}
