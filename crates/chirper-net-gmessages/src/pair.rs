//! Pairing with a phone by QR code, and the web config every session starts from. Port of
//! `libgm/pair.go` (`RegisterPhoneRelay`, `RefreshPhoneRelay`, `RevokeRelayPairing`) and
//! `libgm/client.go` (`FetchConfig`).

use base64::Engine;
use prost::Message;
use serde_json::Value;

use chirper_bridge::{Error, Result};

use crate::api::{Api, Body, PAIRING, browser_details, config_version};
use crate::crypto::{RequestCrypto, uuid_v4};
use crate::proto::pairing::{
    AuthMessage, AuthenticationContainer, EcdsaKeys, KeyData, RefreshPhoneRelayResponse,
    RegisterPhoneRelayResponse, RevokeRelayPairingRequest, RevokeRelayPairingResponse, TokenData,
    UrlData, authentication_container,
};
use crate::pblite::Fields;

/// The page a phone scans to pair with the web client.
pub const QR_URL_BASE: &str = "https://support.google.com/messages/?p=web_computer#?c=";
/// The network name the QR pairing uses in auth messages.
pub const QR_NETWORK: &str = "Bugle";
/// The network name the Google account uses in auth messages.
pub const GOOGLE_NETWORK: &str = "GDitto";

/// The `Config` the web endpoint returns: the client version and the account's device.
#[derive(Clone, PartialEq, prost::Message)]
pub struct Config {
    #[prost(string, tag = "1")]
    pub client_version: String,
    #[prost(message, optional, tag = "5")]
    pub device_info: Option<config::DeviceInfo>,
}

/// Nested types of [`Config`].
pub mod config {
    /// `Config.DeviceInfo`.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct DeviceInfo {
        #[prost(string, tag = "2")]
        pub email: String,
        #[prost(string, tag = "3")]
        pub zero: String,
        #[prost(string, tag = "4")]
        pub device_id: String,
    }
}

/// Read a `Config` from its pblite form.
pub fn config_from_pblite(value: &Value) -> Result<Config> {
    let fields = Fields::of(value)?;
    let device_info = match fields.message(5)? {
        Some(info) => Some(config::DeviceInfo {
            email: info.string(2)?,
            zero: info.string(3)?,
            device_id: info.string(4)?,
        }),
        None => None,
    };
    Ok(Config {
        client_version: fields.string(1)?,
        device_info,
    })
}

/// `GET` the web config, check it is readable, and return the session id it names.
///
/// The session id is the config's `deviceID` as 32 lowercase hex digits (the Go client's `%x` of
/// the UUID), or an empty string when the config has none.
pub async fn fetch_config(api: &Api) -> Result<(Config, String)> {
    let config = match api.get_config().await? {
        Body::Binary(bytes) => Config::decode(bytes.as_slice())
            .map_err(|_| Error::Protocol("the web config is not valid protobuf".to_owned()))?,
        Body::PbLite(value) => config_from_pblite(&value)?,
    };
    let session_id = config
        .device_info
        .as_ref()
        .map(|info| info.device_id.replace('-', "").to_ascii_lowercase())
        .filter(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()))
        .unwrap_or_default();
    Ok((config, session_id))
}

/// Build the QR page URL the phone scans: base64 of `URLData {pairingKey, AESKey, HMACKey}`.
pub fn qr_url(pairing_key: &[u8], crypto: &RequestCrypto) -> Result<String> {
    let (aes, hmac) = crypto.key_pair_bytes();
    let data = UrlData {
        pairing_key: pairing_key.to_vec(),
        aes_key: aes.to_vec(),
        hmac_key: hmac.to_vec(),
    };
    let encoded = base64::engine::general_purpose::STANDARD.encode(data.encode_to_vec());
    Ok(format!("{QR_URL_BASE}{encoded}"))
}

/// The `AuthenticationContainer` of the QR pairing calls.
fn container(
    auth: AuthMessage,
    details: bool,
    data: Option<authentication_container::Data>,
) -> AuthenticationContainer {
    AuthenticationContainer {
        auth_message: Some(auth),
        browser_details: if details { Some(browser_details()) } else { None },
        data,
    }
}

/// A fresh auth header for a pairing request.
fn pairing_auth(token: Vec<u8>) -> Result<AuthMessage> {
    Ok(AuthMessage {
        request_id: uuid_v4()?,
        network: QR_NETWORK.to_owned(),
        tachyon_auth_token: token,
        config_version: Some(config_version()),
    })
}

/// `RegisterPhoneRelay`: start a QR pairing. Stores the token it returns and gives back the pairing key.
pub async fn register_phone_relay(api: &Api) -> Result<RegisterPhoneRelayResponse> {
    let refresh_key = api
        .auth()
        .refresh_key
        .ok_or_else(|| Error::Internal("the session has no refresh key".to_owned()))?;
    let spki = refresh_key.spki_der()?;
    let payload = container(
        pairing_auth(Vec::new())?,
        true,
        Some(authentication_container::Data::KeyData(KeyData {
            mobile: None,
            ecdsa_keys: Some(EcdsaKeys {
                field1: 2,
                encrypted_keys: spki,
            }),
            web_auth_key_data: None,
            browser: None,
        })),
    );
    let url = api.instant_url(PAIRING, "RegisterPhoneRelay");
    let response: RegisterPhoneRelayResponse = api.post_proto(&url, &payload).await?;
    if let Some(token) = response.auth_key_data.clone() {
        store_token(api, token);
    }
    Ok(response)
}

/// `RefreshPhoneRelay`: a new pairing key for the QR code, when the old one ran out.
pub async fn refresh_phone_relay(api: &Api) -> Result<RefreshPhoneRelayResponse> {
    let token = api.auth().tachyon_token;
    let payload = container(pairing_auth(token)?, false, None);
    let url = api.instant_url(PAIRING, "RefreshPhoneRelay");
    api.post_proto(&url, &payload).await
}

/// `RevokeRelayPairing`: remove this browser from the phone.
pub async fn revoke_relay_pairing(api: &Api) -> Result<()> {
    let auth = api.auth();
    let Some(browser) = auth.browser.clone() else {
        return Ok(());
    };
    let payload = RevokeRelayPairingRequest {
        auth_message: Some(pairing_auth(auth.tachyon_token)?),
        browser: Some(browser),
    };
    let url = api.instant_url(PAIRING, "RevokeRelayPairing");
    let _: RevokeRelayPairingResponse = api.post_proto(&url, &payload).await?;
    Ok(())
}

/// Keep the token of a pairing answer (the phone's `PairedData` or the register response).
pub fn store_token(api: &Api, token: TokenData) {
    if token.tachyon_auth_token.is_empty() {
        return;
    }
    api.update_auth(|auth| {
        auth.set_token(token.tachyon_auth_token, token.ttl, crate::link::now_ms());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_qr_url_carries_the_pairing_key_and_the_session_keys() {
        let crypto = RequestCrypto::random().expect("keys");
        let url = qr_url(&[1, 2, 3], &crypto).expect("url");
        let encoded = url.strip_prefix(QR_URL_BASE).expect("prefix");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("base64");
        let data = UrlData::decode(bytes.as_slice()).expect("protobuf");
        assert_eq!(data.pairing_key, vec![1, 2, 3]);
        let (aes, hmac) = crypto.key_pair_bytes();
        assert_eq!(data.aes_key, aes.to_vec());
        assert_eq!(data.hmac_key, hmac.to_vec());
    }

    #[test]
    fn the_config_names_the_session_as_32_hex_digits() {
        let value = serde_json::json!([
            "4.6",
            null,
            null,
            null,
            [null, "someone@example.com", "", "0a1B2c3d-4e5f-6789-abcd-ef0123456789"]
        ]);
        let config = config_from_pblite(&value).expect("parses");
        assert_eq!(config.client_version, "4.6");
        let info = config.device_info.expect("device info");
        assert_eq!(info.email, "someone@example.com");
        assert_eq!(info.device_id.replace('-', "").to_ascii_lowercase().len(), 32);
    }

    #[test]
    fn a_config_without_a_device_has_no_session() {
        let config = config_from_pblite(&serde_json::json!(["4.6"])).expect("parses");
        assert!(config.device_info.is_none());
        assert_eq!(config.client_version, "4.6");
    }

}
