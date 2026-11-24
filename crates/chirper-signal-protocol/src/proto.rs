//! Protobuf message definitions, written by hand as `prost` derives (no build script).
//!
//! Field numbers follow libsignal: `WhisperTextProtocol.proto` (wire messages) and
//! `LocalStorageProtocol.proto` (records). The PQXDH additions (`PreKeySignalMessage` fields 7
//! and 8, `SessionStructure.pending_kyber_pre_key` field 14, Kyber prekey records) follow the
//! Signal Rust implementation. Every structure that holds private key material prints
//! `[redacted]` from `Debug`, because `prost::Message` requires `Debug`.
//!
//! Port of libsignal `serialize/*.proto` (AGPL-3.0-only, Signal Messenger LLC).

/// Wire form of a Double Ratchet message (`SignalMessage`).
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SignalMessage {
    /// Sender's ratchet public key, serialized with the 0x05 type byte.
    #[prost(bytes = "vec", optional, tag = "1")]
    pub ratchet_key: Option<Vec<u8>>,
    /// Index of this message in the sending chain.
    #[prost(uint32, optional, tag = "2")]
    pub counter: Option<u32>,
    /// Length of the previous sending chain.
    #[prost(uint32, optional, tag = "3")]
    pub previous_counter: Option<u32>,
    /// AES-256-CBC ciphertext.
    #[prost(bytes = "vec", optional, tag = "4")]
    pub ciphertext: Option<Vec<u8>>,
}

/// Wire form of a session-establishing message (`PreKeySignalMessage`).
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct PreKeySignalMessage {
    /// Registration id of the sender.
    #[prost(uint32, optional, tag = "5")]
    pub registration_id: Option<u32>,
    /// One-time prekey used, if any.
    #[prost(uint32, optional, tag = "1")]
    pub pre_key_id: Option<u32>,
    /// Signed prekey used.
    #[prost(uint32, optional, tag = "6")]
    pub signed_pre_key_id: Option<u32>,
    /// Sender's base key.
    #[prost(bytes = "vec", optional, tag = "2")]
    pub base_key: Option<Vec<u8>>,
    /// Sender's identity public key.
    #[prost(bytes = "vec", optional, tag = "3")]
    pub identity_key: Option<Vec<u8>>,
    /// Serialized [`SignalMessage`] carried by this prekey message.
    #[prost(bytes = "vec", optional, tag = "4")]
    pub message: Option<Vec<u8>>,
    /// Kyber prekey used (PQXDH, version 4 only).
    #[prost(uint32, optional, tag = "7")]
    pub kyber_pre_key_id: Option<u32>,
    /// Kyber ciphertext of the KEM encapsulation (PQXDH, version 4 only).
    #[prost(bytes = "vec", optional, tag = "8")]
    pub kyber_ciphertext: Option<Vec<u8>>,
}

/// Wire form of a sender-key message (`SenderKeyMessage`).
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SenderKeyMessage {
    /// Sender key id.
    #[prost(uint32, optional, tag = "1")]
    pub id: Option<u32>,
    /// Iteration of the sender chain used.
    #[prost(uint32, optional, tag = "2")]
    pub iteration: Option<u32>,
    /// AES-256-CBC ciphertext.
    #[prost(bytes = "vec", optional, tag = "3")]
    pub ciphertext: Option<Vec<u8>>,
}

/// Wire form of a sender-key distribution message (`SenderKeyDistributionMessage`).
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SenderKeyDistributionMessage {
    /// Sender key id.
    #[prost(uint32, optional, tag = "1")]
    pub id: Option<u32>,
    /// Iteration of the chain key.
    #[prost(uint32, optional, tag = "2")]
    pub iteration: Option<u32>,
    /// Seed of the sender chain key.
    #[prost(bytes = "vec", optional, tag = "3")]
    pub chain_key: Option<Vec<u8>>,
    /// Public signing key of the sender.
    #[prost(bytes = "vec", optional, tag = "4")]
    pub signing_key: Option<Vec<u8>>,
}

/// A chain key and its index (`SessionStructure.Chain.ChainKey`).
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ChainKey {
    /// Index of the key in its chain.
    #[prost(uint32, optional, tag = "1")]
    pub index: Option<u32>,
    /// The 32-byte chain key.
    #[prost(bytes = "vec", optional, tag = "2")]
    pub key: Option<Vec<u8>>,
}

/// Cipher, MAC and IV for a saved message key (`SessionStructure.Chain.MessageKey`).
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct MessageKey {
    /// Counter of the message.
    #[prost(uint32, optional, tag = "1")]
    pub index: Option<u32>,
    /// AES-256 key.
    #[prost(bytes = "vec", optional, tag = "2")]
    pub cipher_key: Option<Vec<u8>>,
    /// HMAC-SHA256 key.
    #[prost(bytes = "vec", optional, tag = "3")]
    pub mac_key: Option<Vec<u8>>,
    /// CBC initialisation vector.
    #[prost(bytes = "vec", optional, tag = "4")]
    pub iv: Option<Vec<u8>>,
}

/// A sending or receiving chain (`SessionStructure.Chain`).
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Chain {
    /// Public ratchet key of the chain.
    #[prost(bytes = "vec", optional, tag = "1")]
    pub sender_ratchet_key: Option<Vec<u8>>,
    /// Private ratchet key (sending chains only).
    #[prost(bytes = "vec", optional, tag = "2")]
    pub sender_ratchet_key_private: Option<Vec<u8>>,
    /// Current chain key.
    #[prost(message, optional, tag = "3")]
    pub chain_key: Option<ChainKey>,
    /// Saved message keys for skipped counters (receiving chains only).
    #[prost(message, repeated, tag = "4")]
    pub message_keys: Vec<MessageKey>,
}

/// A prekey message waiting for acknowledgement (`SessionStructure.PendingPreKey`).
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct PendingPreKey {
    /// One-time prekey id, if one was used.
    #[prost(uint32, optional, tag = "1")]
    pub pre_key_id: Option<u32>,
    /// Signed prekey id.
    #[prost(int32, optional, tag = "3")]
    pub signed_pre_key_id: Option<i32>,
    /// Base key sent in the prekey message.
    #[prost(bytes = "vec", optional, tag = "2")]
    pub base_key: Option<Vec<u8>>,
}

/// A Kyber prekey message waiting for acknowledgement (`SessionStructure.PendingKyberPreKey`).
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct PendingKyberPreKey {
    /// Kyber prekey id.
    #[prost(uint32, optional, tag = "1")]
    pub pre_key_id: Option<u32>,
    /// KEM ciphertext to resend until acknowledged.
    #[prost(bytes = "vec", optional, tag = "2")]
    pub ciphertext: Option<Vec<u8>>,
}

/// One session state (`SessionStructure`).
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SessionStructure {
    /// Message version of the session (3 or 4).
    #[prost(uint32, optional, tag = "1")]
    pub session_version: Option<u32>,
    /// Local identity public key.
    #[prost(bytes = "vec", optional, tag = "2")]
    pub local_identity_public: Option<Vec<u8>>,
    /// Remote identity public key.
    #[prost(bytes = "vec", optional, tag = "3")]
    pub remote_identity_public: Option<Vec<u8>>,
    /// Root key.
    #[prost(bytes = "vec", optional, tag = "4")]
    pub root_key: Option<Vec<u8>>,
    /// Length of the previous sending chain.
    #[prost(uint32, optional, tag = "5")]
    pub previous_counter: Option<u32>,
    /// Current sending chain.
    #[prost(message, optional, tag = "6")]
    pub sender_chain: Option<Chain>,
    /// Receiving chains, newest first.
    #[prost(message, repeated, tag = "7")]
    pub receiver_chains: Vec<Chain>,
    /// Prekey message awaiting acknowledgement.
    #[prost(message, optional, tag = "9")]
    pub pending_pre_key: Option<PendingPreKey>,
    /// Remote registration id.
    #[prost(uint32, optional, tag = "10")]
    pub remote_registration_id: Option<u32>,
    /// Local registration id.
    #[prost(uint32, optional, tag = "11")]
    pub local_registration_id: Option<u32>,
    /// Whether the session needs a refresh (unused, kept for libsignal compatibility).
    #[prost(bool, optional, tag = "12")]
    pub needs_refresh: Option<bool>,
    /// Base key of the session initiator (Alice).
    #[prost(bytes = "vec", optional, tag = "13")]
    pub alice_base_key: Option<Vec<u8>>,
    /// Kyber prekey message awaiting acknowledgement (PQXDH).
    #[prost(message, optional, tag = "14")]
    pub pending_kyber_pre_key: Option<PendingKyberPreKey>,
}

/// A session record: the current state and its archived predecessors (`RecordStructure`).
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct RecordStructure {
    /// The current state.
    #[prost(message, optional, tag = "1")]
    pub current_session: Option<SessionStructure>,
    /// Archived states, newest first.
    #[prost(message, repeated, tag = "2")]
    pub previous_sessions: Vec<SessionStructure>,
}

/// A stored one-time prekey (`PreKeyRecordStructure`).
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct PreKeyRecordStructure {
    /// Prekey id.
    #[prost(uint32, optional, tag = "1")]
    pub id: Option<u32>,
    /// Public key.
    #[prost(bytes = "vec", optional, tag = "2")]
    pub public_key: Option<Vec<u8>>,
    /// Private key.
    #[prost(bytes = "vec", optional, tag = "3")]
    pub private_key: Option<Vec<u8>>,
}

/// A stored signed prekey (`SignedPreKeyRecordStructure`).
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SignedPreKeyRecordStructure {
    /// Prekey id.
    #[prost(uint32, optional, tag = "1")]
    pub id: Option<u32>,
    /// Public key.
    #[prost(bytes = "vec", optional, tag = "2")]
    pub public_key: Option<Vec<u8>>,
    /// Private key.
    #[prost(bytes = "vec", optional, tag = "3")]
    pub private_key: Option<Vec<u8>>,
    /// XEdDSA signature over the serialized public key.
    #[prost(bytes = "vec", optional, tag = "4")]
    pub signature: Option<Vec<u8>>,
    /// Creation time in milliseconds since the Unix epoch.
    #[prost(fixed64, optional, tag = "5")]
    pub timestamp: Option<u64>,
}

/// A stored Kyber prekey (`KyberPreKeyRecordStructure`).
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct KyberPreKeyRecordStructure {
    /// Prekey id.
    #[prost(uint32, optional, tag = "1")]
    pub id: Option<u32>,
    /// Serialized KEM public key (type byte included).
    #[prost(bytes = "vec", optional, tag = "2")]
    pub public_key: Option<Vec<u8>>,
    /// Serialized KEM secret key.
    #[prost(bytes = "vec", optional, tag = "3")]
    pub private_key: Option<Vec<u8>>,
    /// XEdDSA signature over the serialized KEM public key.
    #[prost(bytes = "vec", optional, tag = "4")]
    pub signature: Option<Vec<u8>>,
    /// Creation time in milliseconds since the Unix epoch.
    #[prost(fixed64, optional, tag = "5")]
    pub timestamp: Option<u64>,
}

/// An identity key pair (`IdentityKeyPairStructure`).
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct IdentityKeyPairStructure {
    /// Serialized public key.
    #[prost(bytes = "vec", optional, tag = "1")]
    pub public_key: Option<Vec<u8>>,
    /// Raw private key.
    #[prost(bytes = "vec", optional, tag = "2")]
    pub private_key: Option<Vec<u8>>,
}

/// Sender chain key of a sender-key state (`SenderKeyStateStructure.SenderChainKey`).
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SenderChainKey {
    /// Iteration of the chain key.
    #[prost(uint32, optional, tag = "1")]
    pub iteration: Option<u32>,
    /// The chain key seed.
    #[prost(bytes = "vec", optional, tag = "2")]
    pub seed: Option<Vec<u8>>,
}

/// A saved sender message key (`SenderKeyStateStructure.SenderMessageKey`).
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SenderMessageKey {
    /// Iteration of the key.
    #[prost(uint32, optional, tag = "1")]
    pub iteration: Option<u32>,
    /// Seed the cipher key and IV are derived from.
    #[prost(bytes = "vec", optional, tag = "2")]
    pub seed: Option<Vec<u8>>,
}

/// Signing key pair of a sender-key state (`SenderKeyStateStructure.SenderSigningKey`).
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SenderSigningKey {
    /// Public signing key.
    #[prost(bytes = "vec", optional, tag = "1")]
    pub public: Option<Vec<u8>>,
    /// Private signing key (own sender keys only).
    #[prost(bytes = "vec", optional, tag = "2")]
    pub private: Option<Vec<u8>>,
}

/// One sender-key state (`SenderKeyStateStructure`).
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SenderKeyStateStructure {
    /// Sender key id.
    #[prost(uint32, optional, tag = "1")]
    pub sender_key_id: Option<u32>,
    /// Current chain key.
    #[prost(message, optional, tag = "2")]
    pub sender_chain_key: Option<SenderChainKey>,
    /// Signing key pair.
    #[prost(message, optional, tag = "3")]
    pub sender_signing_key: Option<SenderSigningKey>,
    /// Saved message keys for skipped iterations.
    #[prost(message, repeated, tag = "4")]
    pub sender_message_keys: Vec<SenderMessageKey>,
}

/// All sender-key states for one (group, sender) pair (`SenderKeyRecordStructure`).
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SenderKeyRecordStructure {
    /// States, newest first.
    #[prost(message, repeated, tag = "1")]
    pub sender_key_states: Vec<SenderKeyStateStructure>,
}

/// Decodes a protobuf message, mapping any failure to [`crate::Error::InvalidProtobufEncoding`].
pub(crate) fn decode<T: ::prost::Message + Default>(bytes: &[u8]) -> crate::Result<T> {
    T::decode(bytes).map_err(|_| crate::Error::InvalidProtobufEncoding)
}

/// Returns the value of an optional field, or `Error::InvalidProtobufEncoding` if it is absent.
pub(crate) fn required<T>(field: Option<T>) -> crate::Result<T> {
    field.ok_or(crate::Error::InvalidProtobufEncoding)
}

/// Server certificate inner body (`ServerCertificate.Certificate`).
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ServerCertificateBody {
    /// Key id, used for revocation.
    #[prost(uint32, optional, tag = "1")]
    pub id: Option<u32>,
    /// The server's public key.
    #[prost(bytes = "vec", optional, tag = "2")]
    pub key: Option<Vec<u8>>,
}

/// A server certificate (`ServerCertificate`).
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ServerCertificate {
    /// Serialized [`ServerCertificateBody`].
    #[prost(bytes = "vec", optional, tag = "1")]
    pub certificate: Option<Vec<u8>>,
    /// Trust root signature over `certificate`.
    #[prost(bytes = "vec", optional, tag = "2")]
    pub signature: Option<Vec<u8>>,
}

/// The sender's UUID, as a string or as 16 raw bytes.
#[derive(Clone, PartialEq, ::prost::Oneof)]
pub enum SenderUuid {
    /// Hyphenated UUID string.
    #[prost(string, tag = "6")]
    UuidString(String),
    /// Raw 16-byte UUID.
    #[prost(bytes = "vec", tag = "7")]
    UuidBytes(Vec<u8>),
}

/// Who signed a sender certificate: an embedded server certificate or a key id.
#[derive(Clone, PartialEq, ::prost::Oneof)]
pub enum CertificateSigner {
    /// Serialized [`ServerCertificate`].
    #[prost(bytes = "vec", tag = "5")]
    Certificate(Vec<u8>),
    /// Id of a server key the recipient already knows.
    #[prost(uint32, tag = "8")]
    Id(u32),
}

/// Sender certificate inner body (`SenderCertificate.Certificate`).
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SenderCertificateBody {
    /// E.164 phone number, if any.
    #[prost(string, optional, tag = "1")]
    pub sender_e164: Option<String>,
    /// The sender's UUID.
    #[prost(oneof = "SenderUuid", tags = "6, 7")]
    pub sender_uuid: Option<SenderUuid>,
    /// The sender's device id.
    #[prost(uint32, optional, tag = "2")]
    pub sender_device: Option<u32>,
    /// Expiry in milliseconds since the Unix epoch.
    #[prost(fixed64, optional, tag = "3")]
    pub expires: Option<u64>,
    /// The sender's identity public key.
    #[prost(bytes = "vec", optional, tag = "4")]
    pub identity_key: Option<Vec<u8>>,
    /// Who signed this certificate.
    #[prost(oneof = "CertificateSigner", tags = "5, 8")]
    pub signer: Option<CertificateSigner>,
}

/// A sender certificate (`SenderCertificate`).
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SenderCertificate {
    /// Serialized [`SenderCertificateBody`].
    #[prost(bytes = "vec", optional, tag = "1")]
    pub certificate: Option<Vec<u8>>,
    /// Server signature over `certificate`.
    #[prost(bytes = "vec", optional, tag = "2")]
    pub signature: Option<Vec<u8>>,
}

/// The outer envelope of a sealed sender v1 message (`UnidentifiedSenderMessage`).
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct UnidentifiedSenderMessage {
    /// Ephemeral public key.
    #[prost(bytes = "vec", optional, tag = "1")]
    pub ephemeral_public: Option<Vec<u8>>,
    /// The sender's identity key, encrypted and authenticated.
    #[prost(bytes = "vec", optional, tag = "2")]
    pub encrypted_static: Option<Vec<u8>>,
    /// The inner content, encrypted under the static keys.
    #[prost(bytes = "vec", optional, tag = "3")]
    pub encrypted_message: Option<Vec<u8>>,
}

/// The plaintext inside a sealed sender envelope (`UnidentifiedSenderMessage.Message`).
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct UnidentifiedSenderContent {
    /// Message type: 1 prekey, 2 message, 7 sender key, 8 plaintext.
    #[prost(int32, optional, tag = "1")]
    pub r#type: Option<i32>,
    /// Serialized [`SenderCertificate`].
    #[prost(bytes = "vec", optional, tag = "2")]
    pub sender_certificate: Option<Vec<u8>>,
    /// The wrapped ciphertext (for example a serialized `CiphertextMessage`).
    #[prost(bytes = "vec", optional, tag = "3")]
    pub content: Option<Vec<u8>>,
    /// Content hint: 0 default, 1 resendable, 2 implicit.
    #[prost(int32, optional, tag = "4")]
    pub content_hint: Option<i32>,
    /// Group id, if the message is for a group.
    #[prost(bytes = "vec", optional, tag = "5")]
    pub group_id: Option<Vec<u8>>,
}
