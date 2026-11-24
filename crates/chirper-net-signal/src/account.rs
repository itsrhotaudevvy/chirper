//! The linked account's secrets: identities, password, registration ids and profile key.
//!
//! Stored as JSON under the `session` secret. Byte fields are base64 in the JSON. `Debug` never
//! prints a field.

use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};

/// Everything the driver needs to act as one linked Signal device.
#[derive(Clone, Serialize, Deserialize)]
pub struct Account {
    /// The account identifier (ACI), as a lowercase hyphenated UUID.
    pub aci: String,
    /// The phone-number identifier (PNI), when the account has one.
    #[serde(default)]
    pub pni: Option<String>,
    /// The linked device id; the primary device is 1.
    pub device_id: u32,
    /// The password of the basic-auth pair `aci.device_id:password`.
    pub password: String,
    /// The ACI identity key pair, serialized by the protocol crate.
    #[serde(with = "b64")]
    pub aci_identity: Vec<u8>,
    /// Our ACI registration id.
    pub aci_registration_id: u32,
    /// The 32-byte profile key.
    #[serde(with = "b64")]
    pub profile_key: Vec<u8>,
    /// The account entropy pool (the root of the account's keys), when the server sent one.
    #[serde(default)]
    pub account_entropy_pool: Option<String>,
    /// The phone number, in E.164, when known. Never logged.
    #[serde(default)]
    pub number: Option<String>,
}

impl Account {
    /// The basic-auth user name of this device: `aci.device_id`.
    pub fn username(&self) -> String {
        format!("{}.{}", self.aci, self.device_id)
    }

    /// The `Authorization` header value for the websocket and REST API.
    pub fn basic_auth(&self) -> String {
        basic_auth(&self.username(), &self.password)
    }
}

impl fmt::Debug for Account {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Account")
            .field("aci", &"[redacted]")
            .field("device_id", &self.device_id)
            .field("password", &"[redacted]")
            .finish_non_exhaustive()
    }
}

/// The value of an `authorization:Basic` header for `user:password`.
pub fn basic_auth(user: &str, password: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{user}:{password}")))
}

/// Base64 (standard, padded) for byte fields in JSON.
pub mod b64 {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize, Deserializer, Serializer};

    /// Serializes bytes as a base64 string.
    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    /// Deserializes a base64 string into bytes.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        STANDARD.decode(text).map_err(serde::de::Error::custom)
    }

    /// Encodes bytes as base64 for a request body.
    pub fn encode(bytes: &[u8]) -> String {
        STANDARD.encode(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_username_and_basic_auth_follow_the_reference_format() {
        let account = Account {
            aci: "a0a0a0a0-0000-4000-8000-000000000001".to_owned(),
            pni: None,
            device_id: 2,
            password: "secret".to_owned(),
            aci_identity: vec![1, 2, 3],
            aci_registration_id: 9,
            profile_key: vec![0; 32],
            account_entropy_pool: None,
            number: None,
        };
        assert_eq!(account.username(), "a0a0a0a0-0000-4000-8000-000000000001.2");
        // base64("user:secret") computed once with `printf user:secret | base64`.
        assert_eq!(
            basic_auth("user", "secret"),
            format!("Basic {}", "dXNlcjpzZWNyZXQ=")
        );
        assert!(!format!("{account:?}").contains("secret"));
    }

    #[test]
    fn the_json_round_trips_bytes_as_base64() {
        let account = Account {
            aci: "x".to_owned(),
            pni: Some("PNI:y".to_owned()),
            device_id: 1,
            password: "p".to_owned(),
            aci_identity: vec![0xff, 0x00],
            aci_registration_id: 1,
            profile_key: vec![7; 32],
            account_entropy_pool: None,
            number: None,
        };
        let json = serde_json::to_string(&account).expect("serialize");
        assert!(json.contains("\"aci_identity\":\"/wA=\""));
        let back: Account = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.aci_identity, vec![0xff, 0x00]);
        assert_eq!(back.profile_key, vec![7; 32]);
    }
}
