//! Linking this driver as a secondary device (flow `qr`), and the crypto it needs.
//!
//! Ported from signalmeow's `provisioning.go`, `provisioning_cipher.go` and `devicename.go`
//! (AGPL-3.0). The primary device shows a QR code with an ephemeral key; we answer over the
//! provisioning websocket, decrypt the `ProvisionMessage` it sends back, and register this device
//! with signed and last-resort prekeys and an encrypted device name.

use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, Mac};
use prost::Message as _;
use rand::distributions::Alphanumeric;
use rand::rngs::OsRng;
use rand::Rng;
use serde_json::{Value, json};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use tokio::time::Instant;
use zeroize::Zeroizing;

use chirper_bridge::model::LoginStep;
use chirper_bridge::network::LoginInput;
use chirper_bridge::{Error, LoginCtx, LoginProcess, Result};
use chirper_signal_protocol::curve::{KeyPair, PrivateKey, PublicKey};
use chirper_signal_protocol::identity::{IdentityKey, IdentityKeyPair};
use chirper_signal_protocol::prekey::{
    generate_kyber_pre_key, generate_registration_id, generate_signed_pre_key,
};
use chirper_signal_protocol::cipher;
use chirper_signal_protocol::store::{KyberPreKeyStore, SignedPreKeyStore};

use crate::account::{Account, basic_auth};
use crate::keystore::KeyStore;
use crate::net;
use crate::proto::{DeviceName, ProvisionEnvelope, ProvisionMessage, ProvisioningAddress};
use crate::ws::{self, IncomingRequest, WsClient, WsPump};
use crate::{Endpoints, ids};

/// The method and path of the provisioning request that carries the address.
const ADDRESS_PATH: &str = "/v1/address";
/// The method and path of the provisioning request that carries the message.
const MESSAGE_PATH: &str = "/v1/message";
/// How long the user has to scan the code before the link is abandoned.
const LINK_TIMEOUT: Duration = Duration::from_secs(120);
/// The device name shown in the primary's list of linked devices.
pub const DEVICE_NAME: &str = "Chirper";
/// The capabilities this device declares when linking (no backup import).
pub const LINK_CAPABILITIES: &str = "nopni2";

const PROVISIONING_VERSION: u8 = 1;
const IV_LENGTH: usize = 16;
const MAC_LENGTH: usize = 32;
const HKDF_INFO: &[u8] = b"TextSecure Provisioning Message";

/// Maps a protocol-crate failure: a storage failure is internal, anything else is a protocol
/// error. The messages never carry key material.
pub fn proto(err: chirper_signal_protocol::Error) -> Error {
    match err {
        chirper_signal_protocol::Error::Store(_) => Error::Internal(err.to_string()),
        other => Error::Protocol(other.to_string()),
    }
}

/// HMAC-SHA256 over the concatenation of `parts`.
pub fn hmac_sha256(key: &[u8], parts: &[&[u8]]) -> Result<[u8; 32]> {
    let mut mac = <Hmac<Sha256>>::new_from_slice(key)
        .map_err(|_| Error::Internal("HMAC accepts any key length".to_owned()))?;
    for part in parts {
        mac.update(part);
    }
    Ok(mac.finalize().into_bytes().into())
}

/// AES-256-CTR with a zero IV, as the device-name format uses.
fn aes256_ctr_zero_iv(key: &[u8; 32], data: &[u8]) -> Vec<u8> {
    use ctr::cipher::{KeyIvInit, StreamCipher};
    type Aes256Ctr = ctr::Ctr128BE<aes::Aes256>;
    let mut out = data.to_vec();
    let mut stream = Aes256Ctr::new(key.into(), &[0u8; 16].into());
    stream.apply_keystream(&mut out);
    out
}

/// The ephemeral key pair the primary device encrypts the provisioning message to.
pub struct ProvisioningCipher {
    key_pair: KeyPair,
}

impl ProvisioningCipher {
    /// A cipher with a fresh ephemeral key.
    pub fn new() -> Self {
        Self {
            key_pair: KeyPair::generate(&mut OsRng),
        }
    }

    /// The public key, as the 33-byte serialized form the QR code carries.
    pub fn public_key(&self) -> Vec<u8> {
        self.key_pair.public_key.serialize()
    }

    /// Decrypts a `ProvisionEnvelope` into the account's secrets.
    pub fn decrypt(&self, envelope: &ProvisionEnvelope) -> Result<ProvisionMessage> {
        let their_public = envelope
            .public_key
            .as_deref()
            .ok_or_else(|| Error::Protocol("the provisioning envelope has no key".to_owned()))?;
        let their_public = PublicKey::deserialize(their_public)
            .map_err(|_| Error::Protocol("the provisioning key is not valid".to_owned()))?;
        let body = envelope
            .body
            .as_deref()
            .ok_or_else(|| Error::Protocol("the provisioning envelope has no body".to_owned()))?;
        let minimum = 1 + IV_LENGTH + MAC_LENGTH;
        if body.len() < minimum {
            return Err(Error::Protocol("the provisioning body is too short".to_owned()));
        }
        if body.first() != Some(&PROVISIONING_VERSION) {
            return Err(Error::Protocol("an unknown provisioning version".to_owned()));
        }
        let (signed, mac) = body.split_at(body.len() - MAC_LENGTH);
        let iv = body
            .get(1..1 + IV_LENGTH)
            .ok_or_else(|| Error::Protocol("the provisioning IV is missing".to_owned()))?;
        let ciphertext = signed
            .get(1 + IV_LENGTH..)
            .ok_or_else(|| Error::Protocol("the provisioning ciphertext is missing".to_owned()))?;
        let agreement = self.key_pair.calculate_agreement(&their_public).map_err(proto)?;
        let keys = provisioning_keys(&agreement)?;
        let expected = hmac_sha256(&keys[32..], &[signed])?;
        if !bool::from(expected.as_slice().ct_eq(mac)) {
            return Err(Error::Protocol("the provisioning MAC does not match".to_owned()));
        }
        let key: [u8; 32] = keys[..32]
            .try_into()
            .map_err(|_| Error::Internal("a 32-byte key slice".to_owned()))?;
        let iv: [u8; 16] = iv
            .try_into()
            .map_err(|_| Error::Protocol("the provisioning IV has the wrong size".to_owned()))?;
        let plaintext = cipher::aes_cbc_decrypt(&key, &iv, ciphertext)
            .map_err(|_| Error::Protocol("the provisioning message does not decrypt".to_owned()))?;
        ProvisionMessage::decode(plaintext.as_slice())
            .map_err(|_| Error::Protocol("the provisioning message is not a ProvisionMessage".to_owned()))
    }
}

impl Default for ProvisioningCipher {
    fn default() -> Self {
        Self::new()
    }
}

/// HKDF-SHA256 of the agreement into 64 bytes: the AES key, then the MAC key.
fn provisioning_keys(agreement: &[u8; 32]) -> Result<Zeroizing<[u8; 64]>> {
    let hk = hkdf::Hkdf::<Sha256>::new(None, agreement);
    let mut out = Zeroizing::new([0u8; 64]);
    hk.expand(HKDF_INFO, out.as_mut())
        .map_err(|_| Error::Internal("HKDF output length".to_owned()))?;
    Ok(out)
}

/// Encrypts a device name for the primary device, as `EncryptDeviceName` does.
pub fn encrypt_device_name(name: &str, identity: &IdentityKey) -> Result<Vec<u8>> {
    let ephemeral = KeyPair::generate(&mut OsRng);
    let master = ephemeral
        .calculate_agreement(identity.public_key())
        .map_err(proto)?;
    let name = name.as_bytes();
    let key1 = hmac_sha256(master.as_slice(), &[b"auth"])?;
    let synthetic_iv = hmac_sha256(&key1, &[name])?;
    let synthetic_iv = synthetic_iv[..16].to_vec();
    let key2 = hmac_sha256(master.as_slice(), &[b"cipher"])?;
    let cipher_key = hmac_sha256(&key2, &[&synthetic_iv])?;
    let ciphertext = aes256_ctr_zero_iv(&cipher_key, name);
    Ok(DeviceName {
        ephemeral_public: Some(ephemeral.public_key.serialize()),
        synthetic_iv: Some(synthetic_iv),
        ciphertext: Some(ciphertext),
    }
    .encode_to_vec())
}

/// Decrypts a device name made by [`encrypt_device_name`], checking its synthetic IV.
pub fn decrypt_device_name(wrapped: &[u8], identity: &PrivateKey) -> Result<String> {
    let parts = DeviceName::decode(wrapped)
        .map_err(|_| Error::Protocol("the device name is not a DeviceName".to_owned()))?;
    let field = |value: &Option<Vec<u8>>| {
        value
            .clone()
            .ok_or_else(|| Error::Protocol("a device name field is missing".to_owned()))
    };
    let ephemeral = PublicKey::deserialize(&field(&parts.ephemeral_public)?)
        .map_err(|_| Error::Protocol("the device name key is not valid".to_owned()))?;
    let synthetic_iv = field(&parts.synthetic_iv)?;
    let ciphertext = field(&parts.ciphertext)?;
    let master = identity.calculate_agreement(&ephemeral).map_err(proto)?;
    let key2 = hmac_sha256(master.as_slice(), &[b"cipher"])?;
    let cipher_key = hmac_sha256(&key2, &[&synthetic_iv])?;
    let name = aes256_ctr_zero_iv(&cipher_key, &ciphertext);
    let key1 = hmac_sha256(master.as_slice(), &[b"auth"])?;
    let expected = hmac_sha256(&key1, &[&name])?;
    if !bool::from(expected[..16].ct_eq(&synthetic_iv)) {
        return Err(Error::Protocol("the device name does not authenticate".to_owned()));
    }
    String::from_utf8(name).map_err(|_| Error::Protocol("the device name is not UTF-8".to_owned()))
}

/// The capabilities object the reference declares on linking and on `PUT /v1/devices/capabilities`.
pub fn capabilities(optional_phone_number: bool) -> Value {
    json!({
        "attachmentBackfill": true,
        "spqr": true,
        "usernameChangeSyncMessage": true,
        "optionalPhoneNumber": optional_phone_number,
    })
}

/// A random 22-character password, as the reference generates for each device.
pub fn random_password() -> String {
    OsRng
        .sample_iter(Alphanumeric)
        .take(22)
        .map(char::from)
        .collect()
}

/// Parses a service id given as a string or as its binary form (1 or 17 bytes with a PNI prefix).
fn service_id(text: Option<&str>, binary: Option<&[u8]>) -> Option<String> {
    if let Some(text) = text.filter(|t| !t.is_empty()) {
        return Some(text.to_owned());
    }
    let bytes = binary?;
    match bytes.len() {
        16 => ids::uuid_from_bytes(bytes).ok(),
        17 if bytes.first() == Some(&1) => bytes.get(1..).and_then(|b| ids::uuid_from_bytes(b).ok()).map(|u| format!("PNI:{u}")),
        _ => None,
    }
}

/// The signed prekey as the registration JSON expects it.
pub fn signed_prekey_json(record: &chirper_signal_protocol::prekey::SignedPreKeyRecord) -> Value {
    json!({
        "keyId": record.id(),
        "publicKey": STANDARD.encode(record.public_key().serialize()),
        "signature": STANDARD.encode(record.signature()),
    })
}

/// The Kyber prekey as the registration JSON expects it.
pub fn kyber_prekey_json(record: &chirper_signal_protocol::prekey::KyberPreKeyRecord) -> Value {
    json!({
        "keyId": record.id(),
        "publicKey": STANDARD.encode(record.key_pair().public_key.serialize()),
        "signature": STANDARD.encode(record.signature()),
    })
}

/// One linking attempt: waits for the scan, then registers the device.
pub struct LinkLogin {
    ctx: LoginCtx,
    endpoints: Endpoints,
    stage: Stage,
    done: bool,
}

enum Stage {
    /// Nothing sent yet.
    Start,
    /// The QR code is shown; the provisioning socket waits for the scan.
    Scanning {
        cipher: ProvisioningCipher,
        ws: WsClient,
        pump: WsPump,
        deadline: Instant,
    },
}

impl LinkLogin {
    /// A link attempt for `ctx`, talking to `endpoints`.
    pub fn new(ctx: LoginCtx, endpoints: Endpoints) -> Self {
        Self {
            ctx,
            endpoints,
            stage: Stage::Start,
            done: false,
        }
    }

    /// Opens the provisioning socket and returns the QR step.
    async fn start(&mut self) -> Result<LoginStep> {
        let url = format!("{}/v1/websocket/provisioning/", self.endpoints.ws);
        let socket = net::open(&url, None).await?;
        let (ws, mut pump) = ws::spawn(socket);
        let address = next_request(&mut pump, ADDRESS_PATH).await?;
        let provisioning = ProvisioningAddress::decode(address.body.as_slice())
            .map_err(|_| Error::Protocol("the provisioning address is not valid".to_owned()))?;
        let uuid = provisioning
            .address
            .filter(|a| !a.is_empty())
            .ok_or_else(|| Error::Protocol("the provisioning address is empty".to_owned()))?;
        ws.respond(address.id, 200).await?;
        let cipher = ProvisioningCipher::new();
        let pub_key = STANDARD.encode(cipher.public_key());
        let link = format!(
            "sgnl://linkdevice?uuid={}&pub_key={}&capabilities={}",
            url_encode(&uuid),
            url_encode(&pub_key),
            LINK_CAPABILITIES
        );
        self.stage = Stage::Scanning {
            cipher,
            ws,
            pump,
            deadline: Instant::now() + LINK_TIMEOUT,
        };
        Ok(LoginStep::DisplayAndWait {
            step_id: "fi.chirper.signal.login.qr".to_owned(),
            instructions: "Scan this code with the Signal app: Settings, Linked devices, Link new device."
                .to_owned(),
            kind: "qr".to_owned(),
            data: link,
            image_url: String::new(),
        })
    }

    /// Waits for the scan, registers the device and stores the account.
    async fn finish(&mut self) -> Result<LoginStep> {
        let Stage::Scanning {
            cipher,
            ws,
            mut pump,
            deadline,
        } = std::mem::replace(&mut self.stage, Stage::Start)
        else {
            return Err(Error::Protocol("the link was not started".to_owned()));
        };
        let message = tokio::time::timeout_at(deadline, next_request(&mut pump, MESSAGE_PATH))
            .await
            .map_err(|_| Error::Auth("the QR code was not scanned in time".to_owned()))??;
        let envelope = ProvisionEnvelope::decode(message.body.as_slice())
            .map_err(|_| Error::Protocol("the provisioning envelope is not valid".to_owned()))?;
        ws.respond(message.id, 200).await?;
        let provision = cipher.decrypt(&envelope)?;
        ws.close().await;
        let account = self.register(&provision).await?;
        self.done = true;
        Ok(LoginStep::Complete {
            login_id: account.aci,
        })
    }

    /// Registers this device with the server and stores the account and keys.
    async fn register(&mut self, provision: &ProvisionMessage) -> Result<Account> {
        let identity = identity_pair(
            provision.aci_identity_key_public.as_deref(),
            provision.aci_identity_key_private.as_deref(),
        )?;
        let aci_from_message = service_id(provision.aci.as_deref(), provision.aci_binary.as_deref())
            .ok_or_else(|| Error::Protocol("the provisioning message has no account id".to_owned()))?;
        let mut rng = OsRng;
        let registration_id = generate_registration_id(&mut rng);
        let now = now_ms();
        let signed = generate_signed_pre_key(&identity, 1, now, &mut rng)
            .map_err(|_| Error::Internal("generating a signed prekey".to_owned()))?;
        let kyber = generate_kyber_pre_key(&identity, 1, now, &mut rng)
            .map_err(|_| Error::Internal("generating a Kyber prekey".to_owned()))?;
        let password = random_password();
        let encrypted_name = encrypt_device_name(DEVICE_NAME, &identity.identity_key())?;
        let mut attributes = json!({
            "fetchesMessages": true,
            "name": STANDARD.encode(&encrypted_name),
            "registrationId": registration_id,
            "capabilities": capabilities(provision.pni.is_none()),
        });
        let mut body = json!({
            "verificationCode": provision.provisioning_code.clone().unwrap_or_default(),
            "aciSignedPreKey": signed_prekey_json(&signed),
            "aciPqLastResortPreKey": kyber_prekey_json(&kyber),
        });
        let pni_identity = match provision.pni_identity_key_public.as_deref() {
            Some(_) => Some(identity_pair(
                provision.pni_identity_key_public.as_deref(),
                provision.pni_identity_key_private.as_deref(),
            )?),
            None => None,
        };
        if let Some(pni) = &pni_identity {
            let pni_id = generate_registration_id(&mut rng);
            let pni_signed = generate_signed_pre_key(pni, 1, now, &mut rng)
                .map_err(|_| Error::Internal("generating a PNI prekey".to_owned()))?;
            let pni_kyber = generate_kyber_pre_key(pni, 1, now, &mut rng)
                .map_err(|_| Error::Internal("generating a PNI Kyber prekey".to_owned()))?;
            attributes["pniRegistrationId"] = json!(pni_id);
            body["pniSignedPreKey"] = signed_prekey_json(&pni_signed);
            body["pniPqLastResortPreKey"] = kyber_prekey_json(&pni_kyber);
        }
        body["accountAttributes"] = attributes;
        let payload = serde_json::to_vec(&body)
            .map_err(|_| Error::Internal("encoding the registration".to_owned()))?;
        let socket = net::open(&format!("{}/v1/websocket/", self.endpoints.ws), None).await?;
        let (ws, _pump) = ws::spawn(socket);
        let auth = basic_auth(&aci_from_message, &password);
        let response = ws
            .request(
                "PUT",
                "/v1/devices/link",
                Some(payload),
                vec![
                    "content-type:application/json; charset=utf-8".to_owned(),
                    format!("authorization:{auth}"),
                ],
            )
            .await?;
        ws.close().await;
        let status = response.status.unwrap_or(0);
        if !(200..300).contains(&status) {
            return Err(link_status_error(status));
        }
        let reply: Value = serde_json::from_slice(response.body.as_deref().unwrap_or_default())
            .map_err(|_| Error::Protocol("the link answer is not JSON".to_owned()))?;
        let aci = reply
            .get("uuid")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or(aci_from_message);
        let device_id = reply.get("deviceId").and_then(Value::as_u64).unwrap_or(1);
        let profile_key = provision.profile_key.clone().unwrap_or_default();
        if profile_key.len() != 32 {
            return Err(Error::Protocol("the profile key has the wrong size".to_owned()));
        }
        let account = Account {
            aci: aci.clone(),
            pni: reply.get("pni").and_then(Value::as_str).map(str::to_owned),
            device_id: u32::try_from(device_id)
                .map_err(|_| Error::Protocol("the device id is out of range".to_owned()))?,
            password,
            aci_identity: identity.serialize().to_vec(),
            aci_registration_id: registration_id,
            profile_key,
            account_entropy_pool: provision.account_entropy_pool.clone().filter(|s| !s.is_empty()),
            number: provision.number.clone().filter(|s| !s.is_empty()),
        };
        let mut store = KeyStore::new(&identity, registration_id);
        store
            .store_signed_pre_key(&signed)
            .map_err(|_| Error::Internal("storing the signed prekey".to_owned()))?;
        store
            .store_kyber_pre_key(&kyber)
            .map_err(|_| Error::Internal("storing the Kyber prekey".to_owned()))?;
        store.mark_last_resort(1);
        self.ctx.set_secret("session", &serde_json::to_vec(&account).map_err(|_| Error::Internal("encoding the session".to_owned()))?)?;
        self.ctx.set_secret("store", &store.to_json().map_err(|_| Error::Internal("encoding the keys".to_owned()))?)?;
        Ok(account)
    }
}

/// Reads frames until a server request for `path` arrives; other requests are ignored.
async fn next_request(pump: &mut WsPump, path: &str) -> Result<IncomingRequest> {
    loop {
        let request = pump
            .incoming
            .recv()
            .await
            .ok_or_else(|| Error::Network("the provisioning socket closed".to_owned()))?;
        if request.path == path && request.verb == "PUT" {
            return Ok(request);
        }
    }
}

fn link_status_error(status: u32) -> Error {
    match status {
        401 | 403 => Error::Auth("Signal refused the link".to_owned()),
        429 => Error::RateLimited { retry_after_ms: 60_000 },
        500..=599 => Error::Network(format!("Signal answered {status} to the link")),
        code => Error::Protocol(format!("Signal answered {code} to the link")),
    }
}

fn identity_pair(public: Option<&[u8]>, private: Option<&[u8]>) -> Result<IdentityKeyPair> {
    let public = public.ok_or_else(|| Error::Protocol("an identity key is missing".to_owned()))?;
    let private = private.ok_or_else(|| Error::Protocol("an identity key is missing".to_owned()))?;
    let public = IdentityKey::deserialize(public)
        .map_err(|_| Error::Protocol("an identity public key is not valid".to_owned()))?;
    let private = PrivateKey::deserialize(private)
        .map_err(|_| Error::Protocol("an identity private key is not valid".to_owned()))?;
    Ok(IdentityKeyPair::new(public, private))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Percent-encodes a query component.
fn url_encode(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

#[async_trait]
impl LoginProcess for LinkLogin {
    async fn next(&mut self, _input: LoginInput) -> Result<LoginStep> {
        if self.done {
            return Err(Error::Protocol("the login already finished".to_owned()));
        }
        match self.stage {
            Stage::Start => self.start().await,
            Stage::Scanning { .. } => self.finish().await,
        }
    }

    async fn cancel(&mut self) {
        if let Stage::Scanning { ws, .. } = &self.stage {
            ws.close().await;
        }
        self.stage = Stage::Start;
        let _ = self.ctx.delete_secret("session");
        let _ = self.ctx.delete_secret("store");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    /// Encrypts `message` to `to` the way the primary device does.
    fn seal(to: &PublicKey, message: &ProvisionMessage, rng: &mut StdRng) -> ProvisionEnvelope {
        let ephemeral = KeyPair::generate(rng);
        let agreement = ephemeral.calculate_agreement(to).expect("agreement");
        let keys = provisioning_keys(&agreement).expect("keys");
        let key: [u8; 32] = keys[..32].try_into().expect("key");
        let iv = [9u8; 16];
        let ciphertext = cipher::aes_cbc_encrypt(&key, &iv, &message.encode_to_vec()).expect("cbc");
        let mut body = vec![PROVISIONING_VERSION];
        body.extend_from_slice(&iv);
        body.extend_from_slice(&ciphertext);
        let mac = hmac_sha256(&keys[32..], &[&body]).expect("mac");
        body.extend_from_slice(&mac);
        ProvisionEnvelope {
            public_key: Some(ephemeral.public_key.serialize()),
            body: Some(body),
        }
    }

    fn sample_message() -> ProvisionMessage {
        ProvisionMessage {
            aci: Some("a1b2c3d4-e5f6-4789-8abc-def012345678".to_owned()),
            provisioning_code: Some("123456".to_owned()),
            profile_key: Some(vec![5; 32]),
            account_entropy_pool: Some("pool".to_owned()),
            number: Some("+15550000000".to_owned()),
            ..ProvisionMessage::default()
        }
    }

    #[test]
    fn a_provisioning_message_decrypts_to_the_sent_fields() {
        let mut rng = StdRng::seed_from_u64(11);
        let cipher = ProvisioningCipher {
            key_pair: KeyPair::generate(&mut rng),
        };
        let envelope = seal(&cipher.key_pair.public_key, &sample_message(), &mut rng);
        let decrypted = cipher.decrypt(&envelope).expect("decrypts");
        assert_eq!(decrypted, sample_message());
    }

    #[test]
    fn a_tampered_provisioning_body_is_rejected() {
        let mut rng = StdRng::seed_from_u64(12);
        let cipher = ProvisioningCipher {
            key_pair: KeyPair::generate(&mut rng),
        };
        let mut envelope = seal(&cipher.key_pair.public_key, &sample_message(), &mut rng);
        if let Some(body) = envelope.body.as_mut() {
            let middle = body.len() / 2;
            body[middle] ^= 1;
        }
        assert!(matches!(cipher.decrypt(&envelope), Err(Error::Protocol(_))));
    }

    #[test]
    fn a_provisioning_envelope_for_another_key_is_rejected() {
        let mut rng = StdRng::seed_from_u64(13);
        let cipher = ProvisioningCipher {
            key_pair: KeyPair::generate(&mut rng),
        };
        let other = KeyPair::generate(&mut rng);
        let envelope = seal(&other.public_key, &sample_message(), &mut rng);
        assert!(cipher.decrypt(&envelope).is_err());
    }

    #[test]
    fn a_short_or_empty_provisioning_envelope_is_a_protocol_error() {
        let cipher = ProvisioningCipher::new();
        let empty = ProvisionEnvelope::default();
        assert!(matches!(cipher.decrypt(&empty), Err(Error::Protocol(_))));
        let short = ProvisionEnvelope {
            public_key: Some(cipher.public_key()),
            body: Some(vec![1, 2, 3]),
        };
        assert!(matches!(cipher.decrypt(&short), Err(Error::Protocol(_))));
    }

    #[test]
    fn a_device_name_round_trips_and_rejects_a_wrong_key() {
        let mut rng = StdRng::seed_from_u64(14);
        let identity = IdentityKeyPair::generate(&mut rng);
        let wrapped = encrypt_device_name(DEVICE_NAME, &identity.identity_key()).expect("encrypts");
        let name = decrypt_device_name(&wrapped, identity.private_key()).expect("decrypts");
        assert_eq!(name, DEVICE_NAME);
        let other = IdentityKeyPair::generate(&mut rng);
        assert!(decrypt_device_name(&wrapped, other.private_key()).is_err());
    }

    #[test]
    fn the_capabilities_object_matches_the_reference() {
        let value = capabilities(true);
        assert_eq!(value["optionalPhoneNumber"], json!(true));
        assert_eq!(value["attachmentBackfill"], json!(true));
        assert_eq!(value["spqr"], json!(true));
        assert_eq!(value["usernameChangeSyncMessage"], json!(true));
    }

    #[test]
    fn hmac_matches_rfc4231_test_case_2() {
        // RFC 4231 test case 2: key "Jefe", data "what do ya want for nothing?".
        let mac = hmac_sha256(b"Jefe", &[b"what do ya want for nothing?"]).expect("hmac");
        assert_eq!(
            hex::encode(mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }
}
