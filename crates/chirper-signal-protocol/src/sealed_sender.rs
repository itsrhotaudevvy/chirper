//! Sealed sender version 1 (unidentified delivery): encrypt and decrypt, and the server and
//! sender certificates that authenticate the sender.
//!
//! Port of libsignal `rust/protocol/src/sealed_sender.rs` (the `sealed_sender_v1` module, the
//! certificate types and `UnidentifiedSenderMessageContent`) and `rust/protocol/src/crypto.rs`
//! (AGPL-3.0-only, Signal Messenger LLC). Version 2 (multi-recipient) is not ported.
//!
//! Wire format: `0x11 ‖ UnidentifiedSenderMessage` (protobuf with the ephemeral public key, the
//! encrypted static key and the encrypted content). Each encrypted blob is
//! `AES-256-CTR(zero IV, plaintext) ‖ HMAC-SHA256(mac_key, ciphertext)[..10]`.

use prost::Message;
use rand::{CryptoRng, RngCore};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::curve::{KeyPair, PrivateKey, PublicKey, SIGNATURE_LENGTH};
use crate::error::{Error, Result};
use crate::identity::{IdentityKey, IdentityKeyPair};
use crate::kdf::{hkdf_sha256, hmac_sha256};
use crate::proto::{
    self, CertificateSigner, SenderCertificateBody, SenderUuid, ServerCertificateBody,
    UnidentifiedSenderContent, UnidentifiedSenderMessage as Envelope,
};
use crate::protocol::CiphertextMessageType;

/// The version byte of a sealed sender v1 message: major 1 in the high nibble, and 1.
pub const SEALED_SENDER_V1_VERSION: u8 = 0x11;
/// Length of the truncated HMAC on each encrypted blob.
const MAC_LENGTH: usize = 10;
/// Prefix of the HKDF salt for the ephemeral keys.
const SALT_PREFIX: &[u8] = b"UnidentifiedDelivery";
/// Server key ids that are known to be revoked.
const REVOKED_SERVER_KEY_IDS: &[u32] = &[0xDEAD_C357];

/// Encrypts or decrypts a blob with AES-256-CTR (zero 128-bit counter start) and appends a
/// truncated HMAC-SHA256 over the ciphertext.
fn aes256_ctr_hmacsha256_encrypt(
    plaintext: &[u8],
    cipher_key: &[u8; 32],
    mac_key: &[u8; 32],
) -> Result<Vec<u8>> {
    let mut ciphertext = aes_ctr(plaintext, cipher_key)?;
    let mac = hmac_sha256(mac_key, &[ciphertext.as_slice()])?;
    ciphertext.extend_from_slice(&mac[..MAC_LENGTH]);
    Ok(ciphertext)
}

/// Verifies the trailing MAC in constant time, then decrypts.
fn aes256_ctr_hmacsha256_decrypt(
    sealed: &[u8],
    cipher_key: &[u8; 32],
    mac_key: &[u8; 32],
) -> Result<Vec<u8>> {
    let split = sealed
        .len()
        .checked_sub(MAC_LENGTH)
        .ok_or(Error::InvalidSealedSenderMessage("truncated ciphertext"))?;
    let (ciphertext, their_mac) = sealed.split_at(split);
    let our_mac = hmac_sha256(mac_key, &[ciphertext])?;
    if !bool::from(our_mac[..MAC_LENGTH].ct_eq(their_mac)) {
        return Err(Error::InvalidSealedSenderMessage("MAC verification failed"));
    }
    aes_ctr(ciphertext, cipher_key)
}

fn aes_ctr(data: &[u8], key: &[u8; 32]) -> Result<Vec<u8>> {
    use cbc::cipher::{KeyIvInit, StreamCipher};
    let mut cipher = ctr::Ctr32BE::<aes::Aes256>::new_from_slices(key, &[0u8; 16])
        .map_err(|_| Error::InvalidArgument("aes-ctr key"))?;
    let mut out = data.to_vec();
    cipher.apply_keystream(&mut out);
    Ok(out)
}

/// Splits 96 bytes of HKDF output into three 32-byte keys.
fn split_three(derived: &[u8]) -> Result<([u8; 32], [u8; 32], [u8; 32])> {
    let part = |range: std::ops::Range<usize>| -> Result<[u8; 32]> {
        derived
            .get(range)
            .and_then(|s| s.try_into().ok())
            .ok_or(Error::InvalidArgument("derived key length"))
    };
    Ok((part(0..32)?, part(32..64)?, part(64..96)?))
}

/// The keys that protect the sender's identity key: chain, cipher and MAC keys.
struct EphemeralKeys {
    chain_key: Zeroizing<[u8; 32]>,
    cipher_key: Zeroizing<[u8; 32]>,
    mac_key: Zeroizing<[u8; 32]>,
}

impl EphemeralKeys {
    /// Derives the keys from the DH of `our` and `their`. The salt orders the two public keys
    /// by role, so that sender and recipient compute the same salt.
    fn calculate(our: &KeyPair, their: &PublicKey, sending: bool) -> Result<Self> {
        let our_bytes = our.public_key.serialize();
        let their_bytes = their.serialize();
        let salt = if sending {
            [SALT_PREFIX, their_bytes.as_slice(), our_bytes.as_slice()].concat()
        } else {
            [SALT_PREFIX, our_bytes.as_slice(), their_bytes.as_slice()].concat()
        };
        let shared = our.calculate_agreement(their)?;
        let derived = hkdf_sha256(&shared[..], Some(&salt), &[], 96)?;
        let (chain_key, cipher_key, mac_key) = split_three(&derived)?;
        Ok(Self {
            chain_key: Zeroizing::new(chain_key),
            cipher_key: Zeroizing::new(cipher_key),
            mac_key: Zeroizing::new(mac_key),
        })
    }
}

/// The keys that protect the message content: cipher and MAC keys.
struct StaticKeys {
    cipher_key: Zeroizing<[u8; 32]>,
    mac_key: Zeroizing<[u8; 32]>,
}

impl StaticKeys {
    /// Derives the keys from the identity DH, salted with the chain key and the encrypted static
    /// key. The first 32 bytes of the output are unused.
    fn calculate(
        our: &IdentityKeyPair,
        their: &PublicKey,
        chain_key: &[u8; 32],
        encrypted_static: &[u8],
    ) -> Result<Self> {
        let salt = [&chain_key[..], encrypted_static].concat();
        let shared = our.private_key().calculate_agreement(their)?;
        let derived = hkdf_sha256(&shared[..], Some(&salt), &[], 96)?;
        let (_, cipher_key, mac_key) = split_three(&derived)?;
        Ok(Self {
            cipher_key: Zeroizing::new(cipher_key),
            mac_key: Zeroizing::new(mac_key),
        })
    }
}

/// A server certificate: the server's key, signed by a trust root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerCertificate {
    key_id: u32,
    key: PublicKey,
    certificate: Vec<u8>,
    signature: [u8; SIGNATURE_LENGTH],
    serialized: Vec<u8>,
}

impl ServerCertificate {
    /// Parses the wire form.
    pub fn deserialize(data: &[u8]) -> Result<Self> {
        let outer: proto::ServerCertificate = proto::decode(data)?;
        let certificate = proto::required(outer.certificate)?;
        let signature = <[u8; SIGNATURE_LENGTH]>::try_from(proto::required(outer.signature)?)
            .map_err(|_| Error::InvalidProtobufEncoding)?;
        let body: ServerCertificateBody = proto::decode(&certificate)?;
        Ok(Self {
            key_id: proto::required(body.id)?,
            key: PublicKey::deserialize(&proto::required(body.key)?)?,
            certificate,
            signature,
            serialized: data.to_vec(),
        })
    }

    /// Creates a certificate for `key`, signed by `trust_root`.
    pub fn new<R: CryptoRng + RngCore>(
        key_id: u32,
        key: PublicKey,
        trust_root: &PrivateKey,
        csprng: &mut R,
    ) -> Result<Self> {
        let certificate = ServerCertificateBody {
            id: Some(key_id),
            key: Some(key.serialize()),
        }
        .encode_to_vec();
        let signature = <[u8; SIGNATURE_LENGTH]>::try_from(
            &*trust_root.calculate_signature(&certificate, csprng),
        )
        .map_err(|_| Error::InvalidArgument("signature length"))?;
        let serialized = proto::ServerCertificate {
            certificate: Some(certificate.clone()),
            signature: Some(signature.to_vec()),
        }
        .encode_to_vec();
        Ok(Self {
            key_id,
            key,
            certificate,
            signature,
            serialized,
        })
    }

    /// The key id.
    pub fn key_id(&self) -> u32 {
        self.key_id
    }

    /// The server's public key.
    pub fn key(&self) -> &PublicKey {
        &self.key
    }

    /// The wire form.
    pub fn serialized(&self) -> &[u8] {
        &self.serialized
    }

    /// Whether the trust root signed this certificate and its key id is not revoked.
    pub fn validate(&self, trust_root: &PublicKey) -> bool {
        !REVOKED_SERVER_KEY_IDS.contains(&self.key_id)
            && trust_root.verify_signature(&self.certificate, &self.signature)
    }
}

/// Who signed a sender certificate.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Signer {
    /// The server certificate is embedded in the sender certificate.
    Embedded(ServerCertificate),
    /// Only the id of the signing server key is given.
    Reference(u32),
}

/// A sender certificate: the sender's identity key and address, signed by a server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SenderCertificate {
    signer: Signer,
    key: PublicKey,
    sender_device_id: u32,
    sender_uuid: String,
    sender_e164: Option<String>,
    expiration: u64,
    serialized: Vec<u8>,
    certificate: Vec<u8>,
    signature: [u8; SIGNATURE_LENGTH],
}

/// Formats 16 raw bytes as a hyphenated, lowercase UUID.
fn uuid_string(raw: &[u8]) -> Result<String> {
    if raw.len() != 16 {
        return Err(Error::InvalidProtobufEncoding);
    }
    let hex: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

impl SenderCertificate {
    /// Parses the wire form.
    pub fn deserialize(data: &[u8]) -> Result<Self> {
        let outer: proto::SenderCertificate = proto::decode(data)?;
        let certificate = proto::required(outer.certificate)?;
        let signature = <[u8; SIGNATURE_LENGTH]>::try_from(proto::required(outer.signature)?)
            .map_err(|_| Error::InvalidProtobufEncoding)?;
        let body: SenderCertificateBody = proto::decode(&certificate)?;
        let signer = match proto::required(body.signer)? {
            CertificateSigner::Certificate(bytes) => {
                Signer::Embedded(ServerCertificate::deserialize(&bytes)?)
            }
            CertificateSigner::Id(id) => Signer::Reference(id),
        };
        let sender_uuid = match proto::required(body.sender_uuid)? {
            SenderUuid::UuidString(s) => s,
            SenderUuid::UuidBytes(raw) => uuid_string(&raw)?,
        };
        Ok(Self {
            signer,
            key: PublicKey::deserialize(&proto::required(body.identity_key)?)?,
            sender_device_id: proto::required(body.sender_device)?,
            sender_uuid,
            sender_e164: body.sender_e164,
            expiration: proto::required(body.expires)?,
            serialized: data.to_vec(),
            certificate,
            signature,
        })
    }

    /// Creates a certificate for the sender, signed by `signer_key`, which belongs to `signer`.
    // The arguments are the certificate fields plus the signer; they map one-to-one to the wire.
    #[allow(clippy::too_many_arguments)]
    pub fn new<R: CryptoRng + RngCore>(
        sender_uuid: String,
        sender_e164: Option<String>,
        key: PublicKey,
        sender_device_id: u32,
        expiration: u64,
        signer: ServerCertificate,
        signer_key: &PrivateKey,
        csprng: &mut R,
    ) -> Result<Self> {
        let certificate = SenderCertificateBody {
            sender_e164: sender_e164.clone(),
            sender_uuid: Some(SenderUuid::UuidString(sender_uuid.clone())),
            sender_device: Some(sender_device_id),
            expires: Some(expiration),
            identity_key: Some(key.serialize()),
            signer: Some(CertificateSigner::Certificate(signer.serialized().to_vec())),
        }
        .encode_to_vec();
        let signature = <[u8; SIGNATURE_LENGTH]>::try_from(
            &*signer_key.calculate_signature(&certificate, csprng),
        )
        .map_err(|_| Error::InvalidArgument("signature length"))?;
        let serialized = proto::SenderCertificate {
            certificate: Some(certificate.clone()),
            signature: Some(signature.to_vec()),
        }
        .encode_to_vec();
        Ok(Self {
            signer: Signer::Embedded(signer),
            key,
            sender_device_id,
            sender_uuid,
            sender_e164,
            expiration,
            serialized,
            certificate,
            signature,
        })
    }

    /// The sender's UUID.
    pub fn sender_uuid(&self) -> &str {
        &self.sender_uuid
    }

    /// The sender's E.164 number, if the certificate carries one.
    pub fn sender_e164(&self) -> Option<&str> {
        self.sender_e164.as_deref()
    }

    /// The sender's device id.
    pub fn sender_device_id(&self) -> u32 {
        self.sender_device_id
    }

    /// The sender's identity public key.
    pub fn key(&self) -> &PublicKey {
        &self.key
    }

    /// The wire form.
    pub fn serialized(&self) -> &[u8] {
        &self.serialized
    }

    /// Checks the embedded server certificate against `trust_root`, the server signature over
    /// this certificate, and the expiry against `now_ms` (milliseconds since the Unix epoch).
    /// A certificate that names its signer only by id cannot be validated and is rejected.
    pub fn validate(&self, trust_root: &PublicKey, now_ms: u64) -> bool {
        let Signer::Embedded(server) = &self.signer else {
            return false;
        };
        server.validate(trust_root)
            && server
                .key()
                .verify_signature(&self.certificate, &self.signature)
            && now_ms <= self.expiration
    }
}

/// The message type of a sealed sender content, as the protobuf enum value.
fn proto_message_type(message_type: CiphertextMessageType) -> i32 {
    match message_type {
        CiphertextMessageType::PreKey => 1,
        CiphertextMessageType::Whisper => 2,
        CiphertextMessageType::SenderKey => 7,
        CiphertextMessageType::Plaintext => 8,
    }
}

fn ciphertext_message_type(value: i32) -> Result<CiphertextMessageType> {
    match value {
        1 => Ok(CiphertextMessageType::PreKey),
        2 => Ok(CiphertextMessageType::Whisper),
        7 => Ok(CiphertextMessageType::SenderKey),
        8 => Ok(CiphertextMessageType::Plaintext),
        _ => Err(Error::InvalidSealedSenderMessage("unknown content type")),
    }
}

/// The plaintext of a sealed sender message: the sender's certificate and the wrapped content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnidentifiedSenderMessageContent {
    message_type: CiphertextMessageType,
    sender: SenderCertificate,
    contents: Vec<u8>,
    content_hint: u32,
    group_id: Option<Vec<u8>>,
    serialized: Vec<u8>,
}

impl UnidentifiedSenderMessageContent {
    /// Builds the content. An empty group id is treated as absent, as in libsignal.
    pub fn new(
        message_type: CiphertextMessageType,
        sender: SenderCertificate,
        contents: Vec<u8>,
        content_hint: u32,
        group_id: Option<Vec<u8>>,
    ) -> Result<Self> {
        let group_id = group_id.filter(|g| !g.is_empty());
        let serialized = UnidentifiedSenderContent {
            r#type: Some(proto_message_type(message_type)),
            sender_certificate: Some(sender.serialized().to_vec()),
            content: Some(contents.clone()),
            content_hint: Some(
                i32::try_from(content_hint).map_err(|_| Error::InvalidArgument("content hint"))?,
            ),
            group_id: group_id.clone(),
        }
        .encode_to_vec();
        Ok(Self {
            message_type,
            sender,
            contents,
            content_hint,
            group_id,
            serialized,
        })
    }

    /// Parses the protobuf form.
    pub fn deserialize(data: &[u8]) -> Result<Self> {
        let parsed: UnidentifiedSenderContent = proto::decode(data)?;
        let message_type = ciphertext_message_type(proto::required(parsed.r#type)?)?;
        let sender = SenderCertificate::deserialize(&proto::required(parsed.sender_certificate)?)?;
        Ok(Self {
            message_type,
            sender,
            contents: proto::required(parsed.content)?,
            content_hint: u32::try_from(parsed.content_hint.unwrap_or(0)).unwrap_or(0),
            group_id: parsed.group_id,
            serialized: data.to_vec(),
        })
    }

    /// The wrapped message type.
    pub fn message_type(&self) -> CiphertextMessageType {
        self.message_type
    }

    /// The sender's certificate.
    pub fn sender(&self) -> &SenderCertificate {
        &self.sender
    }

    /// The wrapped ciphertext.
    pub fn contents(&self) -> &[u8] {
        &self.contents
    }

    /// The content hint: 0 default, 1 resendable, 2 implicit.
    pub fn content_hint(&self) -> u32 {
        self.content_hint
    }

    /// The group id, if any.
    pub fn group_id(&self) -> Option<&[u8]> {
        self.group_id.as_deref()
    }

    /// The protobuf form.
    pub fn serialized(&self) -> &[u8] {
        &self.serialized
    }
}

/// Encrypts `content` for `their_identity` as a sealed sender v1 message.
///
/// The sender's identity is hidden from the server; only the recipient can recover it.
pub fn encrypt<R: CryptoRng + RngCore>(
    our_identity: &IdentityKeyPair,
    their_identity: &IdentityKey,
    content: &UnidentifiedSenderMessageContent,
    csprng: &mut R,
) -> Result<Vec<u8>> {
    let ephemeral = KeyPair::generate(csprng);
    let ephemeral_keys = EphemeralKeys::calculate(&ephemeral, their_identity.public_key(), true)?;
    let encrypted_static = aes256_ctr_hmacsha256_encrypt(
        &our_identity.identity_key().serialize(),
        &ephemeral_keys.cipher_key,
        &ephemeral_keys.mac_key,
    )?;
    let static_keys = StaticKeys::calculate(
        our_identity,
        their_identity.public_key(),
        &ephemeral_keys.chain_key,
        &encrypted_static,
    )?;
    let encrypted_message = aes256_ctr_hmacsha256_encrypt(
        content.serialized(),
        &static_keys.cipher_key,
        &static_keys.mac_key,
    )?;
    let mut out = vec![SEALED_SENDER_V1_VERSION];
    out.extend_from_slice(
        &Envelope {
            ephemeral_public: Some(ephemeral.public_key.serialize()),
            encrypted_static: Some(encrypted_static),
            encrypted_message: Some(encrypted_message),
        }
        .encode_to_vec(),
    );
    Ok(out)
}

/// Decrypts the envelope and returns the content, without validating the sender certificate.
///
/// Checks the version byte, both MACs, and that the key in the sender certificate is the key that
/// was encrypted in the envelope. Callers that trust the sender must still call
/// [`SenderCertificate::validate`], or use [`decrypt`].
pub fn decrypt_to_content(
    our_identity: &IdentityKeyPair,
    ciphertext: &[u8],
) -> Result<UnidentifiedSenderMessageContent> {
    let (&version, body) = ciphertext
        .split_first()
        .ok_or(Error::InvalidSealedSenderMessage("message was empty"))?;
    if version != SEALED_SENDER_V1_VERSION {
        return Err(Error::UnknownSealedSenderVersion(version >> 4));
    }
    let envelope: Envelope = proto::decode(body)?;
    let ephemeral_public = PublicKey::deserialize(&proto::required(envelope.ephemeral_public)?)?;
    let encrypted_static = proto::required(envelope.encrypted_static)?;
    let encrypted_message = proto::required(envelope.encrypted_message)?;

    let ephemeral_keys =
        EphemeralKeys::calculate(our_identity.key_pair(), &ephemeral_public, false)?;
    let sender_key_bytes = Zeroizing::new(aes256_ctr_hmacsha256_decrypt(
        &encrypted_static,
        &ephemeral_keys.cipher_key,
        &ephemeral_keys.mac_key,
    )?);
    let sender_key = PublicKey::deserialize(&sender_key_bytes)?;
    let static_keys = StaticKeys::calculate(
        our_identity,
        &sender_key,
        &ephemeral_keys.chain_key,
        &encrypted_static,
    )?;
    let content_bytes = aes256_ctr_hmacsha256_decrypt(
        &encrypted_message,
        &static_keys.cipher_key,
        &static_keys.mac_key,
    )?;
    let content = UnidentifiedSenderMessageContent::deserialize(&content_bytes)?;
    let content_key = content.sender().key().serialize();
    if !bool::from(sender_key_bytes.as_slice().ct_eq(content_key.as_slice())) {
        return Err(Error::InvalidSealedSenderMessage(
            "sender certificate key does not match message key",
        ));
    }
    Ok(content)
}

/// A fully checked sealed sender message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealedSenderMessage {
    /// The sender's UUID, from the validated certificate.
    pub sender_uuid: String,
    /// The sender's E.164 number, if any.
    pub sender_e164: Option<String>,
    /// The sender's device id.
    pub sender_device_id: u32,
    /// The wrapped message type.
    pub message_type: CiphertextMessageType,
    /// The wrapped ciphertext, to be passed to the session cipher.
    pub contents: Vec<u8>,
    /// The content hint.
    pub content_hint: u32,
    /// The group id, if any.
    pub group_id: Option<Vec<u8>>,
}

/// Decrypts a sealed sender v1 message and validates the sender certificate against
/// `trust_root` at time `now_ms`. Fails if the certificate is not valid.
pub fn decrypt(
    our_identity: &IdentityKeyPair,
    ciphertext: &[u8],
    trust_root: &PublicKey,
    now_ms: u64,
) -> Result<SealedSenderMessage> {
    let content = decrypt_to_content(our_identity, ciphertext)?;
    if !content.sender().validate(trust_root, now_ms) {
        return Err(Error::InvalidSealedSenderMessage(
            "sender certificate failed validation",
        ));
    }
    Ok(SealedSenderMessage {
        sender_uuid: content.sender().sender_uuid().to_owned(),
        sender_e164: content.sender().sender_e164().map(str::to_owned),
        sender_device_id: content.sender().sender_device_id(),
        message_type: content.message_type(),
        contents: content.contents().to_vec(),
        content_hint: content.content_hint(),
        group_id: content.group_id().map(<[u8]>::to_vec),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aes_256_ctr_nist_vector() {
        // Ported from libsignal rust/protocol/src/crypto.rs (`aes_ctr_test`), which uses the
        // AES-256 CTR example of NIST SP 800-38A, F.5.5, with 35 zero bytes of plaintext.
        let key: [u8; 32] =
            hex::decode("603DEB1015CA71BE2B73AEF0857D77811F352C073B6108D72D9810A30914DFF4")
                .unwrap()
                .try_into()
                .unwrap();
        let ciphertext = aes_ctr(&[0u8; 35], &key).unwrap();
        assert_eq!(
            hex::encode(ciphertext),
            "e568f68194cf76d6174d4cc04310a85491151e5d0b7a1f1bc0d7acd0ae3e51e4170e23"
        );
    }

    #[test]
    fn sealed_blob_round_trip_and_tag_length() {
        let (key, mac) = ([1u8; 32], [2u8; 32]);
        let sealed = aes256_ctr_hmacsha256_encrypt(b"payload", &key, &mac).unwrap();
        assert_eq!(sealed.len(), b"payload".len() + MAC_LENGTH);
        assert_eq!(
            aes256_ctr_hmacsha256_decrypt(&sealed, &key, &mac).unwrap(),
            b"payload"
        );
        assert!(aes256_ctr_hmacsha256_decrypt(&sealed, &key, &[3u8; 32]).is_err());
        assert!(aes256_ctr_hmacsha256_decrypt(&sealed[..3], &key, &mac).is_err());
    }

    #[test]
    fn uuid_bytes_are_formatted_as_hyphenated_hex() {
        let raw = hex::decode("aaaaaaaa700011ebb32a33b8a8a487a6").unwrap();
        assert_eq!(
            uuid_string(&raw).unwrap(),
            "aaaaaaaa-7000-11eb-b32a-33b8a8a487a6"
        );
        assert!(uuid_string(&raw[..15]).is_err());
    }
}
