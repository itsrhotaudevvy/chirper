//! Pairing a Google account: `SignInGaia` lists the phones the account can pair with, a UKEY2
//! handshake over the messaging channel proves both sides share a code (an emoji the user taps
//! on the phone), and the key schedule derives the session keys. Port of `libgm/pair_google.go`.

use std::time::Duration;

use prost::Message;
use serde_json::Value;
use sha2::{Digest, Sha256, Sha512};
use zeroize::Zeroizing;

use chirper_bridge::{Error, Result};

use crate::api::{Api, REGISTRATION, browser_details, config_version};
use crate::auth::AuthData;
use crate::crypto::{EphemeralP256, Key32, RequestCrypto, hkdf32, random_bytes, uuid_v4};
use crate::emoji;
use crate::proto::pairing::{
    AuthMessage, Device, EcP256PublicKey, GaiaPairingRequestContainer, GaiaPairingResponseContainer,
    GenericPublicKey, SignInGaiaResponse, TokenData, UKEY2_P256_SHA512, Ukey2ClientFinished,
    Ukey2ClientInit, Ukey2Message, Ukey2ServerInit, rpc_gaia_data::Item4, ukey2_client_init,
    ukey2_type,
};
use crate::wire;

/// The network the Google account pairing uses in auth messages.
pub const GOOGLE_NETWORK: &str = "GDitto";
/// How long the phone has to answer the pairing init.
pub const INIT_TIMEOUT: Duration = Duration::from_secs(20);
/// How long the pairing messages live on the server.
const PAIRING_TTL_US: i64 = 300 * 1_000_000;
/// The `PhoneInactive` threshold: a phone not seen for this long is probably switched off.
const PHONE_INACTIVE_AFTER: Duration = Duration::from_secs(48 * 3600);
/// The `private_api_confirmation` string the finish message carries.
const PRIVATE_API_CONFIRMATION: &str = "This is an undocumented API. Use or access of undocumented Google APIs without express authorization is prohibited per the Google API Terms of Service (https://developers.google.com/terms).";
/// The info string of the UKEY2 key schedule (`encryptionKeyInfo` in `pair_google.go`).
const ENCRYPTION_KEY_INFO: [u8; 32] = [
    130, 170, 85, 160, 211, 151, 248, 131, 70, 202, 28, 238, 141, 57, 9, 185, 95, 19, 250, 125, 235,
    29, 74, 179, 131, 118, 184, 37, 109, 168, 85, 16,
];
/// The UKEY2 protocol name the client proposes.
const NEXT_PROTOCOL: &str = "AES_256_CBC-HMAC_SHA256";

/// A phone the Google account can pair with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrimaryDevice {
    /// The registration UUID of the phone.
    pub reg_id: String,
    /// The phone's "unknown int" (its id when it has no timestamp id).
    pub unknown_int: u64,
    /// The phone's "unknown timestamp" id, which the session remembers after pairing.
    pub unknown_ts: u64,
    /// When the phone was last seen, in Unix microseconds (0 when unknown).
    pub last_seen_us: i64,
}

/// The error codes the server sends when the finish step fails (`GaiaPairingErrorCode`).
mod error_code {
    pub const WRONG_VERIFICATION_CODE_SELECTED: i32 = 5;
    pub const VERIFICATION_TIMED_OUT: i32 = 6;
    pub const USER_CANCELED_VERIFICATION: i32 = 7;
    pub const REQUEST_OUT_OF_DATE: i32 = 2;
    pub const REQUEST_NOT_RECEIVED_QUICKLY: i32 = 3;
    pub const USER_DENIED_VERIFICATION_NOT_ME: i32 = 27;
}

/// The pairing attempt in progress: the ephemeral key, the payloads and the server's answer.
pub struct GaiaPairing {
    attempt_id: String,
    started_ms: i64,
    ephemeral: Option<EphemeralP256>,
    dest: PrimaryDevice,
    init_payload: Vec<u8>,
    finish_payload: Vec<u8>,
    server_init: Option<GaiaPairingResponseContainer>,
    next_key: Option<Zeroizing<Vec<u8>>>,
}

impl std::fmt::Debug for GaiaPairing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GaiaPairing")
            .field("dest", &self.dest)
            .finish_non_exhaustive()
    }
}

/// Ask the server for a token and the phones of the account. Stores the token and the browser and
/// mobile identities. The request carries the refresh key so the server can register it.
pub async fn sign_in(api: &Api) -> Result<(SignInGaiaResponse, Vec<PrimaryDevice>)> {
    let auth = api.auth();
    let key = auth
        .refresh_key
        .clone()
        .ok_or_else(|| Error::Internal("the session has no refresh key".to_owned()))?;
    let spki = key.spki_der()?;
    let response = sign_in_request(api, &auth, Some(&spki), 0).await?;
    if let Some(token) = response.token_data.clone() {
        store_token(api, token);
    }
    let device = response
        .device_data
        .as_ref()
        .and_then(|data| data.device_wrapper.as_ref())
        .and_then(|wrapper| wrapper.device.clone())
        .ok_or_else(|| Error::Auth("Google did not name a Messages device for this account".to_owned()))?;
    let mobile = Device {
        source_id: device.source_id.to_ascii_lowercase(),
        ..device.clone()
    };
    api.update_auth(|auth| {
        auth.mobile = Some(mobile);
        auth.browser = Some(device);
    });
    let devices = primary_devices(&response);
    Ok((response, devices))
}

async fn sign_in_request(
    api: &Api,
    auth: &AuthData,
    key: Option<&[u8]>,
    unknown_int3: i64,
) -> Result<SignInGaiaResponse> {
    let device_id = format!("messages-web-{}", auth.session_id);
    let message = AuthMessage {
        request_id: uuid_v4()?,
        network: GOOGLE_NETWORK.to_owned(),
        tachyon_auth_token: Vec::new(),
        config_version: Some(config_version()),
    };
    let body = wire::sign_in_gaia_request_to_pblite(&message, &device_id, key, unknown_int3, GOOGLE_NETWORK);
    let url = format!("{}{REGISTRATION}/SignInGaia", api.google_registration_host());
    let value: Value = api.post_pblite(&url, &body).await?;
    wire::sign_in_gaia_response_from_pblite(&value)
}

fn store_token(api: &Api, token: TokenData) {
    if !token.tachyon_auth_token.is_empty() {
        api.update_auth(|auth| {
            auth.set_token(token.tachyon_auth_token, token.ttl, crate::link::now_ms());
        });
    }
}

/// The phones of a `SignInGaia` answer: the primary devices, with their last-seen times.
pub fn primary_devices(response: &SignInGaiaResponse) -> Vec<PrimaryDevice> {
    let Some(data) = response.device_data.as_ref() else {
        return Vec::new();
    };
    let mut devices: Vec<PrimaryDevice> = data
        .unknown_items2
        .iter()
        .filter(|item| item.unknown_int4 == 1)
        .map(|item| PrimaryDevice {
            reg_id: item.dest_or_source_uuid.clone(),
            unknown_int: item.unknown_big_int7,
            unknown_ts: 0,
            last_seen_us: 0,
        })
        .collect();
    for status in &data.unknown_items3 {
        if let Some(device) = devices.iter_mut().find(|d| d.reg_id == status.dest_or_source_uuid) {
            apply_status(device, status);
        }
    }
    devices
}

fn apply_status(device: &mut PrimaryDevice, status: &Item4) {
    device.last_seen_us = status.unknown_timestamp_microseconds;
    device.unknown_ts = status
        .item8
        .as_ref()
        .map(|item8| item8.unknown_timestamp)
        .unwrap_or(0);
}

/// Choose the phone to pair with: the one the session paired with before, or the most recently
/// seen one. Errors name the reason the account has no usable phone.
pub fn choose_device(
    mut devices: Vec<PrimaryDevice>,
    wanted_phone: u64,
    now_us: i64,
) -> std::result::Result<PrimaryDevice, Error> {
    if devices.is_empty() {
        return Err(Error::Auth(NO_DEVICES.to_owned()));
    }
    devices.sort_by(|a, b| b.last_seen_us.cmp(&a.last_seen_us));
    if wanted_phone != 0 {
        devices.retain(|device| device.unknown_ts == wanted_phone);
        if devices.is_empty() {
            return Err(Error::Auth(PHONE_NOT_FOUND.to_owned()));
        }
    }
    let chosen = devices.swap_remove(0);
    Ok(chosen)
}

/// Whether the phone has not been seen for [`PHONE_INACTIVE_AFTER`]. Only consulted when the phone
/// does not answer the init, to say why.
pub fn is_inactive(device: &PrimaryDevice, now_us: i64) -> bool {
    device.last_seen_us > 0
        && now_us.saturating_sub(device.last_seen_us)
            > i64::try_from(PHONE_INACTIVE_AFTER.as_micros()).unwrap_or(i64::MAX)
}

/// The user-facing messages of a failed pairing, as the Go connector words them.
pub const NO_DEVICES: &str = "No devices found. Make sure you've enabled account pairing in the Google Messages app on your phone.";
/// The phone the session paired with before is no longer on the account.
pub const PHONE_NOT_FOUND: &str = "Your phone isn't ready for pairing. Open Google Messages on your phone, then try again. If you switched to a different phone, remove this account and add it again.";
/// The phone has not been seen for two days.
pub const PHONE_INACTIVE: &str = "Google Messages on your phone hasn't connected to this Google account recently. Open Google Messages on your phone, tap your profile picture, then Device pairing, and make sure it's signed in with the same Google account. Then try again.";
/// The phone did not answer the pairing init.
pub const PHONE_NOT_RESPONDING: &str = "Phone not responding. Make sure your phone is connected to the internet and that account pairing is enabled in the Google Messages app. You may need to keep the app open and/or disable battery optimizations.";

impl GaiaPairing {
    /// Prepare a pairing with `dest`: the ephemeral key and the two UKEY2 payloads.
    pub fn new(dest: PrimaryDevice) -> Result<Self> {
        let ephemeral = EphemeralP256::generate()?;
        let (init_payload, finish_payload) = payloads(&ephemeral)?;
        Ok(Self {
            attempt_id: uuid_v4()?,
            started_ms: crate::link::now_ms(),
            ephemeral: Some(ephemeral),
            dest,
            init_payload,
            finish_payload,
            server_init: None,
            next_key: None,
        })
    }

    /// The phone this pairing is for.
    pub fn dest(&self) -> &PrimaryDevice {
        &self.dest
    }

    /// The `GaiaPairingRequestContainer` for one step.
    pub fn container(&self, data: Vec<u8>, init: bool) -> GaiaPairingRequestContainer {
        let mut container = GaiaPairingRequestContainer {
            pairing_attempt_id: self.attempt_id.clone(),
            browser_details: Some(browser_details()),
            start_timestamp: self.started_ms,
            data,
            proposed_verification_code_version: 0,
            proposed_key_derivation_version: 0,
            private_api_confirmation: String::new(),
        };
        if init {
            container.proposed_verification_code_version = 1;
            container.proposed_key_derivation_version = 1;
        } else {
            container.private_api_confirmation = PRIVATE_API_CONFIRMATION.to_owned();
        }
        container
    }

    /// The attempt's id, which the cancel message names.
    pub fn attempt_id(&self) -> &str {
        &self.attempt_id
    }

    /// The UKEY2 client init bytes (the first request).
    pub fn init_payload(&self) -> &[u8] {
        &self.init_payload
    }

    /// Process the server's init: derive the shared secret and the emoji the phone shows.
    pub fn process_server_init(&mut self, server: GaiaPairingResponseContainer) -> Result<&'static str> {
        let message = Ukey2Message::decode(server.data.as_slice())
            .map_err(|_| protocol("the server init is not valid UKEY2"))?;
        if message.message_type != ukey2_type::SERVER_INIT {
            return Err(protocol("the server sent an unexpected UKEY2 message"));
        }
        let init = Ukey2ServerInit::decode(message.message_data.as_slice())
            .map_err(|_| protocol("the server init payload is not valid UKEY2"))?;
        if init.version != 1 || init.handshake_cipher != UKEY2_P256_SHA512 || init.random.len() != 32 {
            return Err(protocol("the server init has an unsupported version or cipher"));
        }
        let key = init
            .public_key
            .as_ref()
            .and_then(|key| key.ec_p256_public_key.as_ref())
            .ok_or_else(|| protocol("the server init has no public key"))?;
        let peer = uncompressed_point(&key.x, &key.y)?;
        let ephemeral = self
            .ephemeral
            .take()
            .ok_or_else(|| Error::Internal("the pairing key was already used".to_owned()))?;
        let shared = ephemeral.agree(&peer)?;
        let shared_secret = Sha256::digest(shared.as_slice());
        let mut auth_info = self.init_payload.clone();
        auth_info.extend_from_slice(&server.data);
        let auth = hkdf32(&shared_secret, b"UKEY2 v1 auth", &auth_info)?;
        let next = hkdf32(&shared_secret, b"UKEY2 v1 next", &auth_info)?;
        self.next_key = Some(Zeroizing::new(next.to_vec()));
        let auth_number = u32::from_be_bytes([auth[0], auth[1], auth[2], auth[3]]);
        let version = server.confirmed_verification_code_version;
        self.server_init = Some(server);
        emoji::pairing_emoji(version, auth_number)
            .ok_or_else(|| protocol("the server asked for an unknown verification code version"))
    }

    /// The finish message bytes (the second request).
    pub fn finish_payload(&self) -> &[u8] {
        &self.finish_payload
    }

    /// Turn the finish answer into the session keys. Returns the phone id the login is named by.
    pub fn derive_keys(&self, finish: &GaiaPairingResponseContainer, mobile: &str) -> Result<(RequestCrypto, String)> {
        if finish.finish_error_type != 0 {
            return Err(finish_error(finish));
        }
        let next = self
            .next_key
            .as_ref()
            .ok_or_else(|| Error::Internal("the server init was not processed".to_owned()))?;
        let server = self
            .server_init
            .as_ref()
            .ok_or_else(|| Error::Internal("the server init was not processed".to_owned()))?;
        let client_key = hkdf32(next, &ENCRYPTION_KEY_INFO, b"client")?;
        let server_key = hkdf32(next, &ENCRYPTION_KEY_INFO, b"server")?;
        let crypto = match server.confirmed_key_derivation_version {
            0 => RequestCrypto::from_keys(client_key, server_key),
            1 => {
                let mut concatenated = Vec::with_capacity(96);
                concatenated.extend_from_slice(&ENCRYPTION_KEY_INFO);
                if byte_hash(client_key.as_slice()) < byte_hash(server_key.as_slice()) {
                    concatenated.extend_from_slice(client_key.as_slice());
                    concatenated.extend_from_slice(server_key.as_slice());
                } else {
                    concatenated.extend_from_slice(server_key.as_slice());
                    concatenated.extend_from_slice(client_key.as_slice());
                }
                let digest = Sha256::digest(&concatenated);
                let aes: Key32 = hkdf32(&digest, b"Ditto salt 1", b"Ditto info 1")?;
                let hmac: Key32 = hkdf32(&digest, b"Ditto salt 2", b"Ditto info 2")?;
                RequestCrypto::from_keys(aes, hmac)
            }
            other => {
                return Err(Error::Protocol(format!(
                    "the server asked for an unsupported key derivation version {other}"
                )));
            }
        };
        let phone = if self.dest.unknown_ts != 0 {
            self.dest.unknown_ts
        } else {
            self.dest.unknown_int
        };
        Ok((crypto, format!("{mobile}/{phone}")))
    }

}

/// Map a failed finish answer onto the user-facing error.
pub fn finish_error(finish: &GaiaPairingResponseContainer) -> Error {
    match finish.finish_error_code {
        error_code::WRONG_VERIFICATION_CODE_SELECTED => {
            Error::Auth("Incorrect emoji chosen on phone, please try again".to_owned())
        }
        error_code::USER_CANCELED_VERIFICATION | error_code::USER_DENIED_VERIFICATION_NOT_ME => {
            Error::Auth("Pairing cancelled on phone".to_owned())
        }
        error_code::REQUEST_OUT_OF_DATE
        | error_code::REQUEST_NOT_RECEIVED_QUICKLY
        | error_code::VERIFICATION_TIMED_OUT => {
            Error::Auth("Pairing timed out, please try again".to_owned())
        }
        other => Error::Protocol(format!(
            "Google failed the pairing with code {}/{other}",
            finish.finish_error_type
        )),
    }
}

/// The `GaiaPairingRequestContainer`-free part of the pairing: the two UKEY2 messages.
fn payloads(ephemeral: &EphemeralP256) -> Result<(Vec<u8>, Vec<u8>)> {
    let point = ephemeral.public_point();
    if point.len() != 65 || point[0] != 4 {
        return Err(Error::Internal("the ECDH public key is malformed".to_owned()));
    }
    let key = GenericPublicKey {
        kind: 1,
        ec_p256_public_key: Some(EcP256PublicKey {
            x: [&[0u8][..], &point[1..33]].concat(),
            y: [&[0u8][..], &point[33..65]].concat(),
        }),
    };
    let finished = Ukey2ClientFinished {
        public_key: Some(key),
    };
    let finish = Ukey2Message {
        message_type: ukey2_type::CLIENT_FINISH,
        message_data: finished.encode_to_vec(),
    }
    .encode_to_vec();
    let commitment = Sha512::digest(&finish);
    let init = Ukey2ClientInit {
        version: 1,
        random: random_bytes(32)?,
        cipher_commitments: vec![ukey2_client_init::CipherCommitment {
            handshake_cipher: UKEY2_P256_SHA512,
            commitment: commitment.to_vec(),
        }],
        next_protocol: NEXT_PROTOCOL.to_owned(),
    };
    let init = Ukey2Message {
        message_type: ukey2_type::CLIENT_INIT,
        message_data: init.encode_to_vec(),
    }
    .encode_to_vec();
    Ok((init, finish))
}

/// The uncompressed P-256 point of the server's coordinates, which may carry a leading zero byte.
fn uncompressed_point(x: &[u8], y: &[u8]) -> Result<Vec<u8>> {
    let x = strip_zero(x)?;
    let y = strip_zero(y)?;
    if x.len() > 32 || y.len() > 32 {
        return Err(protocol("the server's public key coordinates are too long"));
    }
    let mut point = vec![0u8; 65];
    point[0] = 4;
    point[33 - x.len()..33].copy_from_slice(x);
    point[65 - y.len()..].copy_from_slice(y);
    Ok(point)
}

fn strip_zero(coordinate: &[u8]) -> Result<&[u8]> {
    match coordinate.len() {
        33 if coordinate[0] == 0 => Ok(&coordinate[1..]),
        33 => Err(protocol("the server's public key has an unexpected prefix")),
        _ => Ok(coordinate),
    }
}

/// The byte hash of the Go client (`byteHash`): a signed 32-bit polynomial, compared to order keys.
fn byte_hash(bytes: &[u8]) -> i32 {
    bytes.iter().fold(1i32, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(i32::from(*byte as i8))
    })
}

fn protocol(message: &str) -> Error {
    Error::Protocol(message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(reg: &str, unknown_int: u64, unknown_ts: u64, seen: i64) -> PrimaryDevice {
        PrimaryDevice {
            reg_id: reg.to_owned(),
            unknown_int,
            unknown_ts,
            last_seen_us: seen,
        }
    }

    #[test]
    fn the_most_recently_seen_phone_is_chosen() {
        let chosen = choose_device(
            vec![device("a", 1, 10, 100), device("b", 2, 20, 300)],
            0,
            400,
        )
        .expect("a device");
        assert_eq!(chosen.reg_id, "b");
    }

    #[test]
    fn a_remembered_phone_must_still_be_on_the_account() {
        let err = choose_device(vec![device("a", 1, 10, 100)], 99, 200).expect_err("missing");
        assert!(matches!(err, Error::Auth(message) if message == PHONE_NOT_FOUND));
        let chosen = choose_device(vec![device("a", 1, 10, 100)], 10, 200).expect("found");
        assert_eq!(chosen.reg_id, "a");
    }

    #[test]
    fn an_account_without_phones_says_so() {
        let err = choose_device(Vec::new(), 0, 0).expect_err("none");
        assert!(matches!(err, Error::Auth(message) if message == NO_DEVICES));
    }

    #[test]
    fn a_phone_not_seen_for_two_days_is_inactive() {
        let two_days_us = 49 * 3600 * 1_000_000i64;
        assert!(is_inactive(&device("a", 1, 10, 1), two_days_us));
        assert!(!is_inactive(&device("a", 1, 10, 0), two_days_us));
    }

    #[test]
    fn coordinates_with_a_leading_zero_are_accepted() {
        let x = [&[0u8][..], &[0xffu8; 32][..]].concat();
        let y = vec![0x11u8; 32];
        let point = uncompressed_point(&x, &y).expect("point");
        assert_eq!(point.len(), 65);
        assert_eq!(point[0], 4);
        assert_eq!(point[1], 0xff);
        assert_eq!(point[33], 0x11);
        assert!(uncompressed_point(&[1u8; 33], &y).is_err());
    }

    #[test]
    fn byte_hash_matches_the_go_polynomial() {
        // out = 1; out = 31*out + int8(b) for each byte.
        assert_eq!(byte_hash(&[]), 1);
        assert_eq!(byte_hash(&[1]), 32);
        assert_eq!(byte_hash(&[0xff]), 31 - 1);
    }

    #[test]
    fn a_pairing_offers_two_ukey2_messages() {
        let pairing = GaiaPairing::new(device("a", 1, 10, 1)).expect("pairing");
        let init = Ukey2Message::decode(pairing.init_payload()).expect("init");
        assert_eq!(init.message_type, ukey2_type::CLIENT_INIT);
        let client_init = Ukey2ClientInit::decode(init.message_data.as_slice()).expect("payload");
        assert_eq!(client_init.version, 1);
        assert_eq!(client_init.random.len(), 32);
        assert_eq!(client_init.next_protocol, NEXT_PROTOCOL);
        let commitment = &client_init.cipher_commitments[0].commitment;
        assert_eq!(
            commitment.as_slice(),
            Sha512::digest(pairing.finish_payload()).as_slice()
        );
        let finish = Ukey2Message::decode(pairing.finish_payload()).expect("finish");
        assert_eq!(finish.message_type, ukey2_type::CLIENT_FINISH);
        let key = Ukey2ClientFinished::decode(finish.message_data.as_slice())
            .expect("payload")
            .public_key
            .expect("key");
        assert_eq!(key.ec_p256_public_key.expect("ec").x[0], 0);
    }

    #[test]
    fn a_failed_finish_maps_to_the_user_facing_message() {
        let finish = GaiaPairingResponseContainer {
            finish_error_type: 1,
            finish_error_code: error_code::WRONG_VERIFICATION_CODE_SELECTED,
            ..Default::default()
        };
        assert!(matches!(finish_error(&finish), Error::Auth(m) if m.contains("Incorrect emoji")));
    }
}
