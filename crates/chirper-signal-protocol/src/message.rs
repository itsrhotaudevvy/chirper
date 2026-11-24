//! Wire messages: `SignalMessage`, `PreKeySignalMessage`, `SenderKeyMessage`,
//! `SenderKeyDistributionMessage` and the `CiphertextMessage` enum.
//!
//! Port of libsignal `protocol/{SignalMessage,PreKeySignalMessage,SenderKeyMessage,
//! SenderKeyDistributionMessage,CiphertextMessage}.go` and `serialize/ProtoBufferSerializer.go`
//! (AGPL-3.0-only, Signal Messenger LLC).
//!
//! Layout: one version byte, then the protobuf body, then (for signed or MACed messages) a
//! trailer. The version byte is `(v << 4) | v` for message version `v`. For version 3 that is
//! the ASCII digit `'3'` that libsignal Go writes, so the bytes match the reference exactly.
//! Parsing reads only the high nibble, as libsignal does.
//!
//! * `SignalMessage`: trailer is an 8-byte truncated `HMAC-SHA256(mac_key, sender_identity ‖
//!   receiver_identity ‖ version_byte ‖ body)`.
//! * `SenderKeyMessage`: trailer is a 64-byte XEdDSA signature over `version_byte ‖ body`.

use std::fmt;

use prost::Message;
use rand::{CryptoRng, RngCore};
use subtle::ConstantTimeEq;

use crate::curve::{PrivateKey, PublicKey, SIGNATURE_LENGTH};
use crate::error::{Error, Result};
use crate::identity::IdentityKey;
use crate::kdf::hmac_sha256;
use crate::proto::{
    self, PreKeySignalMessage as PreKeyProto, SenderKeyDistributionMessage as SkdmProto,
    SenderKeyMessage as SenderKeyProto, SignalMessage as SignalProto,
};
use crate::protocol::CiphertextMessageType;

/// Length of the truncated MAC on a `SignalMessage`.
pub const MAC_LENGTH: usize = 8;
/// Lowest message version this crate reads.
pub const MIN_MESSAGE_VERSION: u32 = 3;
/// Highest message version this crate reads (PQXDH).
pub const MAX_MESSAGE_VERSION: u32 = 4;
/// The message version of group (sender key) messages.
pub const SENDER_KEY_MESSAGE_VERSION: u32 = 3;

/// Builds the version byte for a message version: `(v << 4) | v`.
fn version_byte(version: u32) -> Result<u8> {
    if !(MIN_MESSAGE_VERSION..=MAX_MESSAGE_VERSION).contains(&version) {
        return Err(Error::UnrecognizedMessageVersion(version));
    }
    u8::try_from((version << 4) | version).map_err(|_| Error::UnrecognizedMessageVersion(version))
}

/// Reads the message version from the high nibble of the first byte.
fn read_version(byte: u8) -> Result<u32> {
    match u32::from(byte >> 4) {
        v @ MIN_MESSAGE_VERSION..=MAX_MESSAGE_VERSION => Ok(v),
        v @ 0..=2 => Err(Error::LegacyCiphertextVersion(v as u8)),
        v => Err(Error::UnrecognizedCiphertextVersion(v as u8)),
    }
}

/// Splits `bytes` into `(version, body)` after checking there is a version byte and at least
/// `trailer` more bytes.
fn split_header(bytes: &[u8], trailer: usize) -> Result<(u32, &[u8])> {
    let too_short = Error::CiphertextMessageTooShort(bytes.len());
    let (&first, rest) = bytes.split_first().ok_or_else(|| too_short.clone())?;
    let body_len = rest
        .len()
        .checked_sub(trailer)
        .ok_or_else(|| too_short.clone())?;
    let body = rest.get(..body_len).ok_or(too_short)?;
    Ok((read_version(first)?, body))
}

/// Checks the Kyber fields against the message version: present in 4, absent in 3.
fn kyber_fields(
    version: u32,
    id: Option<u32>,
    ciphertext: Option<Vec<u8>>,
) -> Result<Option<(u32, Vec<u8>)>> {
    match (version, id, ciphertext) {
        (SENDER_KEY_MESSAGE_VERSION, None, None) => Ok(None),
        (SENDER_KEY_MESSAGE_VERSION, _, _) => Err(Error::InvalidMessage(
            CiphertextMessageType::PreKey,
            "kyber fields in a version 3 message",
        )),
        (_, Some(id), Some(ciphertext)) => Ok(Some((id, ciphertext))),
        _ => Err(Error::InvalidMessage(
            CiphertextMessageType::PreKey,
            "missing kyber fields in a version 4 message",
        )),
    }
}

/// Computes the truncated MAC over `version ‖ body` with both identity keys bound in.
fn signal_mac(
    mac_key: &[u8; 32],
    sender: &IdentityKey,
    receiver: &IdentityKey,
    unsigned: &[u8],
) -> Result<[u8; MAC_LENGTH]> {
    let sender_bytes = sender.serialize();
    let receiver_bytes = receiver.serialize();
    let full = hmac_sha256(mac_key, &[&sender_bytes, &receiver_bytes, unsigned])?;
    let mut out = [0u8; MAC_LENGTH];
    out.copy_from_slice(&full[..MAC_LENGTH]);
    Ok(out)
}

/// A Double Ratchet message (`SignalMessage`).
#[derive(Clone, PartialEq, Eq)]
pub struct SignalMessage {
    message_version: u32,
    sender_ratchet_key: PublicKey,
    counter: u32,
    previous_counter: u32,
    ciphertext: Vec<u8>,
    serialized: Vec<u8>,
}

impl SignalMessage {
    /// Builds and MACs a message. `serialized` is `version ‖ body ‖ mac`.
    // The arguments are the wire fields plus the MAC inputs; a struct would only rename them.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        message_version: u32,
        counter: u32,
        previous_counter: u32,
        mac_key: &[u8; 32],
        sender_ratchet_key: PublicKey,
        ciphertext: Vec<u8>,
        sender_identity: &IdentityKey,
        receiver_identity: &IdentityKey,
    ) -> Result<Self> {
        let body = SignalProto {
            ratchet_key: Some(sender_ratchet_key.serialize()),
            counter: Some(counter),
            previous_counter: Some(previous_counter),
            ciphertext: Some(ciphertext.clone()),
        }
        .encode_to_vec();
        let mut unsigned = Vec::with_capacity(1 + body.len() + MAC_LENGTH);
        unsigned.push(version_byte(message_version)?);
        unsigned.extend_from_slice(&body);
        let mac = signal_mac(mac_key, sender_identity, receiver_identity, &unsigned)?;
        unsigned.extend_from_slice(&mac);
        Ok(Self {
            message_version,
            sender_ratchet_key,
            counter,
            previous_counter,
            ciphertext,
            serialized: unsigned,
        })
    }

    /// Parses the wire form. The MAC is not checked here; call [`SignalMessage::verify_mac`].
    pub fn deserialize(bytes: &[u8]) -> Result<Self> {
        let (message_version, body) = split_header(bytes, MAC_LENGTH)?;
        let parsed: SignalProto = proto::decode(body)?;
        let sender_ratchet_key = PublicKey::deserialize(&proto::required(parsed.ratchet_key)?)?;
        Ok(Self {
            message_version,
            sender_ratchet_key,
            counter: proto::required(parsed.counter)?,
            previous_counter: parsed.previous_counter.unwrap_or(0),
            ciphertext: proto::required(parsed.ciphertext)?,
            serialized: bytes.to_vec(),
        })
    }

    /// The complete wire form, including the version byte and MAC.
    pub fn serialized(&self) -> &[u8] {
        &self.serialized
    }

    /// The message version (3 or 4).
    pub fn message_version(&self) -> u32 {
        self.message_version
    }

    /// The sender's ratchet public key.
    pub fn sender_ratchet_key(&self) -> &PublicKey {
        &self.sender_ratchet_key
    }

    /// The counter of this message in its sending chain.
    pub fn counter(&self) -> u32 {
        self.counter
    }

    /// The length of the sender's previous sending chain.
    pub fn previous_counter(&self) -> u32 {
        self.previous_counter
    }

    /// The AES-256-CBC ciphertext.
    pub fn ciphertext(&self) -> &[u8] {
        &self.ciphertext
    }

    /// Checks the trailing MAC in constant time against the identity keys and `mac_key`.
    pub fn verify_mac(
        &self,
        sender_identity: &IdentityKey,
        receiver_identity: &IdentityKey,
        mac_key: &[u8; 32],
    ) -> Result<()> {
        let split = self
            .serialized
            .len()
            .checked_sub(MAC_LENGTH)
            .ok_or(Error::CiphertextMessageTooShort(self.serialized.len()))?;
        let (unsigned, received) = self.serialized.split_at(split);
        let expected = signal_mac(mac_key, sender_identity, receiver_identity, unsigned)?;
        if bool::from(expected.ct_eq(received)) {
            Ok(())
        } else {
            Err(Error::InvalidMessage(
                CiphertextMessageType::Whisper,
                "bad MAC",
            ))
        }
    }
}

impl fmt::Debug for SignalMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignalMessage")
            .field("version", &self.message_version)
            .field("counter", &self.counter)
            .finish_non_exhaustive()
    }
}

/// A session-establishing message (`PreKeySignalMessage`) wrapping a [`SignalMessage`].
#[derive(Clone, PartialEq, Eq)]
pub struct PreKeySignalMessage {
    message_version: u32,
    registration_id: u32,
    pre_key_id: Option<u32>,
    signed_pre_key_id: u32,
    base_key: PublicKey,
    identity_key: IdentityKey,
    message: SignalMessage,
    kyber_pre_key_id: Option<u32>,
    kyber_ciphertext: Option<Vec<u8>>,
    serialized: Vec<u8>,
}

/// The fields of a [`PreKeySignalMessage`] that the sender chooses.
pub struct PreKeySignalParts<'a> {
    /// Message version (3 or 4).
    pub message_version: u32,
    /// Sender's registration id.
    pub registration_id: u32,
    /// One-time prekey id used, if any.
    pub pre_key_id: Option<u32>,
    /// Signed prekey id used.
    pub signed_pre_key_id: u32,
    /// Sender's base key.
    pub base_key: PublicKey,
    /// Sender's identity key.
    pub identity_key: IdentityKey,
    /// The wrapped Double Ratchet message.
    pub message: &'a SignalMessage,
    /// Kyber prekey id and KEM ciphertext (version 4 only).
    pub kyber: Option<(u32, Vec<u8>)>,
}

impl PreKeySignalMessage {
    /// Builds the wire form and re-parses it, so the result is exactly what a receiver sees.
    pub fn new(parts: PreKeySignalParts<'_>) -> Result<Self> {
        let (kyber_pre_key_id, kyber_ciphertext) = match parts.kyber {
            Some((id, ciphertext)) => (Some(id), Some(ciphertext)),
            None => (None, None),
        };
        let body = PreKeyProto {
            registration_id: Some(parts.registration_id),
            pre_key_id: parts.pre_key_id,
            signed_pre_key_id: Some(parts.signed_pre_key_id),
            base_key: Some(parts.base_key.serialize()),
            identity_key: Some(parts.identity_key.serialize()),
            message: Some(parts.message.serialized().to_vec()),
            kyber_pre_key_id,
            kyber_ciphertext,
        }
        .encode_to_vec();
        let mut bytes = Vec::with_capacity(1 + body.len());
        bytes.push(version_byte(parts.message_version)?);
        bytes.extend_from_slice(&body);
        Self::deserialize(&bytes)
    }

    /// Parses the wire form, enforcing the version rules for the Kyber fields.
    pub fn deserialize(bytes: &[u8]) -> Result<Self> {
        let (message_version, body) = split_header(bytes, 0)?;
        let parsed: PreKeyProto = proto::decode(body)?;
        let kyber = kyber_fields(
            message_version,
            parsed.kyber_pre_key_id,
            parsed.kyber_ciphertext.clone(),
        )?;
        let message = SignalMessage::deserialize(&proto::required(parsed.message)?)?;
        Ok(Self {
            message_version,
            registration_id: proto::required(parsed.registration_id)?,
            pre_key_id: parsed.pre_key_id,
            signed_pre_key_id: proto::required(parsed.signed_pre_key_id)?,
            base_key: PublicKey::deserialize(&proto::required(parsed.base_key)?)?,
            identity_key: IdentityKey::deserialize(&proto::required(parsed.identity_key)?)?,
            message,
            kyber_pre_key_id: kyber.as_ref().map(|(id, _)| *id),
            kyber_ciphertext: kyber.map(|(_, ciphertext)| ciphertext),
            serialized: bytes.to_vec(),
        })
    }

    /// The complete wire form.
    pub fn serialized(&self) -> &[u8] {
        &self.serialized
    }

    /// The message version (3 or 4).
    pub fn message_version(&self) -> u32 {
        self.message_version
    }

    /// The sender's registration id.
    pub fn registration_id(&self) -> u32 {
        self.registration_id
    }

    /// The one-time prekey id used, if any.
    pub fn pre_key_id(&self) -> Option<u32> {
        self.pre_key_id
    }

    /// The signed prekey id used.
    pub fn signed_pre_key_id(&self) -> u32 {
        self.signed_pre_key_id
    }

    /// The sender's base key.
    pub fn base_key(&self) -> &PublicKey {
        &self.base_key
    }

    /// The sender's identity key.
    pub fn identity_key(&self) -> &IdentityKey {
        &self.identity_key
    }

    /// The wrapped Double Ratchet message.
    pub fn message(&self) -> &SignalMessage {
        &self.message
    }

    /// The Kyber prekey id, present only in version 4 messages.
    pub fn kyber_pre_key_id(&self) -> Option<u32> {
        self.kyber_pre_key_id
    }

    /// The KEM ciphertext, present only in version 4 messages.
    pub fn kyber_ciphertext(&self) -> Option<&[u8]> {
        self.kyber_ciphertext.as_deref()
    }
}

impl fmt::Debug for PreKeySignalMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreKeySignalMessage")
            .field("version", &self.message_version)
            .field("signed_pre_key_id", &self.signed_pre_key_id)
            .field("pre_key_id", &self.pre_key_id)
            .finish_non_exhaustive()
    }
}

/// A group message (`SenderKeyMessage`), signed by the sender's chain signing key.
#[derive(Clone, PartialEq, Eq)]
pub struct SenderKeyMessage {
    key_id: u32,
    iteration: u32,
    ciphertext: Vec<u8>,
    serialized: Vec<u8>,
}

impl SenderKeyMessage {
    /// Builds and signs a group message.
    pub fn new<R: CryptoRng + RngCore>(
        key_id: u32,
        iteration: u32,
        ciphertext: Vec<u8>,
        signing_key: &PrivateKey,
        csprng: &mut R,
    ) -> Result<Self> {
        let body = SenderKeyProto {
            id: Some(key_id),
            iteration: Some(iteration),
            ciphertext: Some(ciphertext.clone()),
        }
        .encode_to_vec();
        let mut unsigned = Vec::with_capacity(1 + body.len() + SIGNATURE_LENGTH);
        unsigned.push(version_byte(SENDER_KEY_MESSAGE_VERSION)?);
        unsigned.extend_from_slice(&body);
        let signature = signing_key.calculate_signature(&unsigned, csprng);
        unsigned.extend_from_slice(&signature);
        Ok(Self {
            key_id,
            iteration,
            ciphertext,
            serialized: unsigned,
        })
    }

    /// Parses the wire form. The signature is not checked here.
    pub fn deserialize(bytes: &[u8]) -> Result<Self> {
        let (version, body) = split_header(bytes, SIGNATURE_LENGTH)?;
        if version != SENDER_KEY_MESSAGE_VERSION {
            return Err(Error::UnrecognizedMessageVersion(version));
        }
        let parsed: SenderKeyProto = proto::decode(body)?;
        Ok(Self {
            key_id: proto::required(parsed.id)?,
            iteration: proto::required(parsed.iteration)?,
            ciphertext: proto::required(parsed.ciphertext)?,
            serialized: bytes.to_vec(),
        })
    }

    /// The complete wire form, including the signature.
    pub fn serialized(&self) -> &[u8] {
        &self.serialized
    }

    /// The sender key id.
    pub fn key_id(&self) -> u32 {
        self.key_id
    }

    /// The iteration of the message key used.
    pub fn iteration(&self) -> u32 {
        self.iteration
    }

    /// The AES-256-CBC ciphertext.
    pub fn ciphertext(&self) -> &[u8] {
        &self.ciphertext
    }

    /// Verifies the trailing XEdDSA signature over everything before it.
    pub fn verify_signature(&self, signing_key: &PublicKey) -> Result<()> {
        let split = self
            .serialized
            .len()
            .checked_sub(SIGNATURE_LENGTH)
            .ok_or(Error::CiphertextMessageTooShort(self.serialized.len()))?;
        let (unsigned, signature) = self.serialized.split_at(split);
        if signing_key.verify_signature(unsigned, signature) {
            Ok(())
        } else {
            Err(Error::SignatureValidationFailed)
        }
    }
}

impl fmt::Debug for SenderKeyMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SenderKeyMessage")
            .field("key_id", &self.key_id)
            .field("iteration", &self.iteration)
            .finish_non_exhaustive()
    }
}

/// The message that shares a sender chain with the members of a group (`SenderKeyDistributionMessage`).
#[derive(Clone, PartialEq, Eq)]
pub struct SenderKeyDistributionMessage {
    key_id: u32,
    iteration: u32,
    chain_key: [u8; 32],
    signing_key: PublicKey,
    serialized: Vec<u8>,
}

impl SenderKeyDistributionMessage {
    /// Builds the wire form for a sender chain.
    pub fn new(
        key_id: u32,
        iteration: u32,
        chain_key: [u8; 32],
        signing_key: PublicKey,
    ) -> Result<Self> {
        let body = SkdmProto {
            id: Some(key_id),
            iteration: Some(iteration),
            chain_key: Some(chain_key.to_vec()),
            signing_key: Some(signing_key.serialize()),
        }
        .encode_to_vec();
        let mut serialized = Vec::with_capacity(1 + body.len());
        serialized.push(version_byte(SENDER_KEY_MESSAGE_VERSION)?);
        serialized.extend_from_slice(&body);
        Ok(Self {
            key_id,
            iteration,
            chain_key,
            signing_key,
            serialized,
        })
    }

    /// Parses the wire form.
    pub fn deserialize(bytes: &[u8]) -> Result<Self> {
        let (version, body) = split_header(bytes, 0)?;
        if version != SENDER_KEY_MESSAGE_VERSION {
            return Err(Error::UnrecognizedMessageVersion(version));
        }
        let parsed: SkdmProto = proto::decode(body)?;
        Ok(Self {
            key_id: proto::required(parsed.id)?,
            iteration: proto::required(parsed.iteration)?,
            chain_key: crate::session::key32(proto::required(parsed.chain_key)?)?,
            signing_key: PublicKey::deserialize(&proto::required(parsed.signing_key)?)?,
            serialized: bytes.to_vec(),
        })
    }

    /// The complete wire form.
    pub fn serialized(&self) -> &[u8] {
        &self.serialized
    }

    /// The sender key id.
    pub fn key_id(&self) -> u32 {
        self.key_id
    }

    /// The iteration of the chain key.
    pub fn iteration(&self) -> u32 {
        self.iteration
    }

    /// The seed of the sender chain key.
    pub fn chain_key(&self) -> &[u8; 32] {
        &self.chain_key
    }

    /// The sender's public signing key.
    pub fn signing_key(&self) -> &PublicKey {
        &self.signing_key
    }
}

impl fmt::Debug for SenderKeyDistributionMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SenderKeyDistributionMessage")
            .field("key_id", &self.key_id)
            .field("iteration", &self.iteration)
            .finish_non_exhaustive()
    }
}

/// Any one-to-one ciphertext: a session message or a session-establishing message.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CiphertextMessage {
    /// A Double Ratchet message.
    Signal(SignalMessage),
    /// A session-establishing message.
    PreKey(PreKeySignalMessage),
}

impl CiphertextMessage {
    /// The complete wire form.
    pub fn serialized(&self) -> &[u8] {
        match self {
            CiphertextMessage::Signal(m) => m.serialized(),
            CiphertextMessage::PreKey(m) => m.serialized(),
        }
    }

    /// The wire type of this message.
    pub fn message_type(&self) -> CiphertextMessageType {
        match self {
            CiphertextMessage::Signal(_) => CiphertextMessageType::Whisper,
            CiphertextMessage::PreKey(_) => CiphertextMessageType::PreKey,
        }
    }
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    use super::*;
    use crate::curve::KeyPair;
    use crate::identity::IdentityKeyPair;

    fn sample_signal(version: u32) -> (SignalMessage, IdentityKey, IdentityKey) {
        let mut rng = StdRng::seed_from_u64(20);
        let a = IdentityKeyPair::generate(&mut rng).identity_key();
        let b = IdentityKeyPair::generate(&mut rng).identity_key();
        let ratchet = KeyPair::generate(&mut rng);
        let msg = SignalMessage::new(
            version,
            7,
            3,
            &[9u8; 32],
            ratchet.public_key,
            vec![0xAA; 32],
            &a,
            &b,
        )
        .unwrap();
        (msg, a, b)
    }

    #[test]
    fn version_byte_matches_libsignal_go() {
        // Go writes strconv.Itoa(3) == "3" == 0x33 for version 3.
        assert_eq!(version_byte(3).unwrap(), b'3');
        assert_eq!(version_byte(4).unwrap(), 0x44);
        assert!(version_byte(5).is_err());
        assert_eq!(read_version(0x34).unwrap(), 3);
        assert!(matches!(
            read_version(0x20),
            Err(Error::LegacyCiphertextVersion(2))
        ));
        assert!(matches!(
            read_version(0x50),
            Err(Error::UnrecognizedCiphertextVersion(5))
        ));
    }

    #[test]
    fn signal_message_round_trip_and_mac() {
        let (msg, a, b) = sample_signal(3);
        assert_eq!(msg.serialized()[0], b'3');
        let parsed = SignalMessage::deserialize(msg.serialized()).unwrap();
        assert_eq!(parsed, msg);
        assert_eq!(parsed.counter(), 7);
        assert_eq!(parsed.previous_counter(), 3);
        parsed.verify_mac(&a, &b, &[9u8; 32]).unwrap();
        // The MAC binds the identity order: swapping sender and receiver must fail.
        assert!(parsed.verify_mac(&b, &a, &[9u8; 32]).is_err());
        assert!(parsed.verify_mac(&a, &b, &[8u8; 32]).is_err());
    }

    #[test]
    fn signal_message_known_answer() {
        // Computed independently in Python (hmac/hashlib plus a hand-written protobuf encoder):
        // version 3, counter 7, previous 3, MAC key 0x09*32, sender identity 05||11*32,
        // receiver identity 05||33*32, ratchet 05||22*32, ciphertext 0xAA*32.
        let key = |tag: u8| {
            let mut k = vec![0x05];
            k.extend_from_slice(&[tag; 32]);
            IdentityKey::deserialize(&k).unwrap()
        };
        let ratchet = PublicKey::deserialize(&{
            let mut k = vec![0x05];
            k.extend_from_slice(&[0x22; 32]);
            k
        })
        .unwrap();
        let msg = SignalMessage::new(
            3,
            7,
            3,
            &[9u8; 32],
            ratchet,
            vec![0xAA; 32],
            &key(0x11),
            &key(0x33),
        )
        .unwrap();
        assert_eq!(
            hex::encode(msg.serialized()),
            "330a21052222222222222222222222222222222222222222222222222222222222222222100718032220aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaab562ec0235150c1d"
        );
        msg.verify_mac(&key(0x11), &key(0x33), &[9u8; 32]).unwrap();
    }

    #[test]
    fn truncated_and_flipped_signal_messages_never_panic() {
        let (msg, a, b) = sample_signal(4);
        let bytes = msg.serialized().to_vec();
        for len in 0..bytes.len() {
            let _ = SignalMessage::deserialize(&bytes[..len])
                .and_then(|m| m.verify_mac(&a, &b, &[9u8; 32]));
        }
        for i in 0..bytes.len() * 8 {
            let mut bad = bytes.clone();
            bad[i / 8] ^= 1 << (i % 8);
            let parsed = SignalMessage::deserialize(&bad);
            if let Ok(parsed) = parsed {
                assert!(
                    parsed.verify_mac(&a, &b, &[9u8; 32]).is_err(),
                    "bit {i} accepted"
                );
            }
        }
    }

    #[test]
    fn prekey_message_round_trip_and_version_rules() {
        let (msg, a, _) = sample_signal(3);
        let mut rng = StdRng::seed_from_u64(21);
        let base = KeyPair::generate(&mut rng);
        let pre = PreKeySignalMessage::new(PreKeySignalParts {
            message_version: 3,
            registration_id: 1234,
            pre_key_id: Some(5),
            signed_pre_key_id: 6,
            base_key: base.public_key,
            identity_key: a.clone(),
            message: &msg,
            kyber: None,
        })
        .unwrap();
        assert_eq!(pre.serialized()[0], b'3');
        let parsed = PreKeySignalMessage::deserialize(pre.serialized()).unwrap();
        assert_eq!(parsed.registration_id(), 1234);
        assert_eq!(parsed.pre_key_id(), Some(5));
        assert_eq!(parsed.signed_pre_key_id(), 6);
        assert_eq!(parsed.message(), &msg);
        assert!(parsed.kyber_ciphertext().is_none());

        // Version 4 requires the Kyber fields, and version 3 forbids them.
        let (msg4, _, _) = sample_signal(4);
        let v4 = PreKeySignalMessage::new(PreKeySignalParts {
            message_version: 4,
            registration_id: 1,
            pre_key_id: None,
            signed_pre_key_id: 2,
            base_key: base.public_key,
            identity_key: a.clone(),
            message: &msg4,
            kyber: Some((9, vec![1u8; 1569])),
        })
        .unwrap();
        assert_eq!(v4.kyber_pre_key_id(), Some(9));
        assert_eq!(v4.kyber_ciphertext().unwrap().len(), 1569);
        let no_kyber = PreKeySignalMessage::new(PreKeySignalParts {
            message_version: 4,
            registration_id: 1,
            pre_key_id: None,
            signed_pre_key_id: 2,
            base_key: base.public_key,
            identity_key: a.clone(),
            message: &msg4,
            kyber: None,
        });
        assert!(no_kyber.is_err());
    }

    #[test]
    fn sender_key_message_sign_and_verify() {
        let mut rng = StdRng::seed_from_u64(22);
        let signing = KeyPair::generate(&mut rng);
        let msg =
            SenderKeyMessage::new(11, 4, vec![3u8; 48], &signing.private_key, &mut rng).unwrap();
        let parsed = SenderKeyMessage::deserialize(msg.serialized()).unwrap();
        assert_eq!((parsed.key_id(), parsed.iteration()), (11, 4));
        parsed.verify_signature(&signing.public_key).unwrap();
        let other = KeyPair::generate(&mut rng);
        assert!(parsed.verify_signature(&other.public_key).is_err());
    }

    #[test]
    fn sender_key_distribution_round_trip() {
        let mut rng = StdRng::seed_from_u64(23);
        let signing = KeyPair::generate(&mut rng);
        let skdm = SenderKeyDistributionMessage::new(77, 0, [4u8; 32], signing.public_key).unwrap();
        let parsed = SenderKeyDistributionMessage::deserialize(skdm.serialized()).unwrap();
        assert_eq!(parsed, skdm);
        assert_eq!(parsed.chain_key(), &[4u8; 32]);
        assert_eq!(parsed.signing_key(), &signing.public_key);
    }

    #[test]
    fn debug_hides_ciphertext() {
        let (msg, _, _) = sample_signal(3);
        assert!(!format!("{msg:?}").contains("aa"));
    }
}
