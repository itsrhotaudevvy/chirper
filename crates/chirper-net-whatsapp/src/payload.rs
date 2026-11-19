//! The `ClientPayload` a connection sends in its Noise finish: the registration payload for a new
//! device, and the login payload for a paired one.
//!
//! Field values follow the web client as wacore 0.7.0 (MIT),
//! https://github.com/jlucaso1/whatsapp-rust builds them (`Device::get_client_payload`,
//! `build_base_client_payload`): the web platform, the release channel, the `"0.1.0"` OS
//! string and the `en`/`US` locale, with the `Desktop` device name.

use chirper_bridge::{Error, Result};
use md5::{Digest, Md5};
use prost::Message;

use crate::keys::{DeviceKeys, SIGNED_PRE_KEY_ID};
use crate::proto::{ClientPayload, DeviceProps, client_payload, device_props};

/// The OS label the device registers under.
pub const DEVICE_OS: &str = "Chirper";

/// The payload for registering a new device.
pub fn registration(keys: &DeviceKeys, version: (u32, u32, u32)) -> Result<ClientPayload> {
    let (primary, secondary, tertiary) = version;
    let version_text = format!("{primary}.{secondary}.{tertiary}");
    let build_hash = Md5::digest(version_text.as_bytes());
    let device_props = DeviceProps {
        os: Some(DEVICE_OS.to_owned()),
        version: Some(device_props::AppVersion {
            primary: Some(primary),
            secondary: Some(secondary),
            tertiary: Some(tertiary),
            ..Default::default()
        }),
        platform_type: Some(device_props::platform_type::CHROME),
        require_full_sync: Some(false),
    };
    let key_id = SIGNED_PRE_KEY_ID.to_be_bytes();
    let pairing = client_payload::DevicePairingRegistrationData {
        e_regid: Some(keys.registration_id.to_be_bytes().to_vec()),
        e_keytype: Some(vec![5]),
        e_ident: Some(keys.identity.public().to_vec()),
        e_skey_id: Some(key_id[1..].to_vec()),
        e_skey_val: Some(keys.signed_pre_key.public().to_vec()),
        e_skey_sig: Some(keys.signed_pre_key_signature.to_vec()),
        build_hash: Some(build_hash.to_vec()),
        device_props: Some(device_props.encode_to_vec()),
    };
    Ok(ClientPayload {
        device_pairing_data: Some(pairing),
        passive: Some(false),
        pull: Some(false),
        ..base(version)
    })
}

/// The payload for logging a paired device in: the account's phone number and device number.
pub fn login(keys: &DeviceKeys, version: (u32, u32, u32)) -> Result<ClientPayload> {
    let jid = keys
        .jid
        .as_ref()
        .ok_or_else(|| Error::Auth("this device is not paired".to_owned()))?;
    let username = jid
        .user
        .parse::<u64>()
        .map_err(|_| Error::Protocol("the paired account has no phone number".to_owned()))?;
    Ok(ClientPayload {
        username: Some(username),
        device: Some(u32::from(jid.device)),
        passive: Some(false),
        pull: Some(true),
        lc: Some(0),
        lid_db_migrated: Some(false),
        ..base(version)
    })
}

/// The fields both payloads share: the user agent, web info and connect flags.
fn base(version: (u32, u32, u32)) -> ClientPayload {
    let (primary, secondary, tertiary) = version;
    ClientPayload {
        user_agent: Some(client_payload::UserAgent {
            platform: Some(client_payload::user_agent::platform::WEB),
            app_version: Some(client_payload::user_agent::AppVersion {
                primary: Some(primary),
                secondary: Some(secondary),
                tertiary: Some(tertiary),
                ..Default::default()
            }),
            mcc: Some("000".to_owned()),
            mnc: Some("000".to_owned()),
            os_version: Some("0.1.0".to_owned()),
            manufacturer: Some(String::new()),
            device: Some("Desktop".to_owned()),
            os_build_number: Some("0.1.0".to_owned()),
            release_channel: Some(client_payload::user_agent::release_channel::RELEASE),
            locale_language_iso6391: Some("en".to_owned()),
            locale_country_iso31661_alpha2: Some("US".to_owned()),
            ..Default::default()
        }),
        web_info: Some(client_payload::WebInfo {
            web_sub_platform: Some(0),
        }),
        connect_type: Some(client_payload::connect_type::WIFI_UNKNOWN),
        connect_reason: Some(client_payload::connect_reason::USER_ACTIVATED),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_VERSION: (u32, u32, u32) = (2, 3000, 1_042_742_319);

    #[test]
    fn registration_carries_the_pairing_data() {
        let keys = DeviceKeys::generate().expect("keys");
        let payload = registration(&keys, TEST_VERSION).expect("builds");
        let pairing = payload.device_pairing_data.expect("pairing data");
        assert_eq!(pairing.e_regid.expect("regid").len(), 4);
        assert_eq!(pairing.e_keytype, Some(vec![5]));
        assert_eq!(pairing.e_skey_id, Some(vec![0, 0, 1]));
        assert_eq!(pairing.build_hash.expect("hash").len(), 16);
        assert_eq!(payload.passive, Some(false));
        assert_eq!(payload.pull, Some(false));
        assert!(payload.username.is_none());

        let props =
            DeviceProps::decode(pairing.device_props.expect("props").as_slice()).expect("decodes");
        assert_eq!(props.os.as_deref(), Some(DEVICE_OS));
        assert_eq!(
            props.platform_type,
            Some(device_props::platform_type::CHROME)
        );
    }

    #[test]
    fn build_hash_is_md5_of_the_app_version() {
        let keys = DeviceKeys::generate().expect("keys");
        let payload = registration(&keys, TEST_VERSION).expect("builds");
        let hash = payload
            .device_pairing_data
            .expect("pairing")
            .build_hash
            .expect("hash");
        assert_eq!(hash, Md5::digest(b"2.3000.1042742319").to_vec());
    }

    #[test]
    fn login_uses_the_account_and_device() {
        let mut keys = DeviceKeys::generate().expect("keys");
        assert!(matches!(login(&keys, TEST_VERSION), Err(Error::Auth(_))));
        keys.jid = Some("15551234567:3@s.whatsapp.net".parse().expect("jid"));
        let payload = login(&keys, TEST_VERSION).expect("builds");
        assert_eq!(payload.username, Some(15_551_234_567));
        assert_eq!(payload.device, Some(3));
        assert_eq!(payload.pull, Some(true));
        assert!(payload.device_pairing_data.is_none());
    }

    #[test]
    fn payloads_survive_encoding() {
        let keys = DeviceKeys::generate().expect("keys");
        let bytes = registration(&keys, TEST_VERSION)
            .expect("builds")
            .encode_to_vec();
        assert!(ClientPayload::decode(bytes.as_slice()).is_ok());
    }
}
