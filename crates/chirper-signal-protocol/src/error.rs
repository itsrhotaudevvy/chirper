//! Error type for the protocol library.
//!
//! The variants follow libsignal's `SignalProtocolError` (rust/protocol/src/error.rs). Display
//! strings never include keys, message bodies, or account names.

use std::fmt;

use crate::ProtocolAddress;
use crate::protocol::CiphertextMessageType;

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// A failure reported by a driver-supplied store.
///
/// The operation name is a fixed string so that no record contents can leak into the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreError {
    /// The store operation that failed, for example `"load_session"`.
    pub operation: &'static str,
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "storage backend failed during {}", self.operation)
    }
}

impl std::error::Error for StoreError {}

/// Errors produced by the protocol library. Mirrors libsignal's `SignalProtocolError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// invalid argument: {0}
    InvalidArgument(&'static str),
    /// invalid state for call to {0} to succeed: {1}
    InvalidState(&'static str, &'static str),
    /// protobuf encoding was invalid
    InvalidProtobufEncoding,
    /// ciphertext serialized bytes were too short
    CiphertextMessageTooShort(usize),
    /// ciphertext version was too old <{0}>
    LegacyCiphertextVersion(u8),
    /// ciphertext version was unrecognized <{0}>
    UnrecognizedCiphertextVersion(u8),
    /// unrecognized message version <{0}>
    UnrecognizedMessageVersion(u32),
    /// no key type identifier
    NoKeyTypeIdentifier,
    /// bad key type <{0:#04x}>
    BadKeyType(u8),
    /// bad key length <{1}> for key with type <{0}>
    BadKeyLength(KeyType, usize),
    /// invalid key agreement (all-zero shared secret)
    InvalidKeyAgreement,
    /// invalid signature detected
    SignatureValidationFailed,
    /// untrusted identity
    UntrustedIdentity(ProtocolAddress),
    /// invalid prekey identifier
    InvalidPreKeyId,
    /// invalid signed prekey identifier
    InvalidSignedPreKeyId,
    /// invalid Kyber prekey identifier
    InvalidKyberPreKeyId,
    /// invalid MAC key length <{0}>
    InvalidMacKeyLength(usize),
    /// missing sender key state
    NoSenderKeyState,
    /// no session for the given address
    SessionNotFound(ProtocolAddress),
    /// invalid session structure: {0}
    InvalidSessionStructure(&'static str),
    /// invalid sender key session
    InvalidSenderKeySession,
    /// invalid registration id
    InvalidRegistrationId,
    /// message with old counter {0} / {1}
    DuplicatedMessage(u32, u32),
    /// invalid {0:?} message: {1}
    InvalidMessage(CiphertextMessageType, &'static str),
    /// invalid sealed sender message: {0}
    InvalidSealedSenderMessage(&'static str),
    /// unknown sealed sender message version {0}
    UnknownSealedSenderVersion(u8),
    /// self send of a sealed sender message
    SealedSenderSelfSend,
    /// unknown server certificate ID {0}
    UnknownSealedSenderServerCertificateId(u32),
    /// bad KEM key type <{0:#04x}>
    BadKemKeyType(u8),
    /// unexpected KEM key type <{0:#04x}> (expected <{1:#04x}>)
    WrongKemKeyType(u8, u8),
    /// bad KEM key length <{1}> for key with type <{0:?}>
    BadKemKeyLength(crate::kem::KeyType, usize),
    /// bad KEM ciphertext length <{1}> for key with type <{0:?}>
    BadKemCiphertextLength(crate::kem::KeyType, usize),
    /// a store operation failed
    Store(StoreError),
}

/// Key types recognised by [`crate::curve`]. Only DJB (Curve25519) keys exist today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum KeyType {
    /// Curve25519 keys in Daniel J. Bernstein's format, serialized with a 0x05 prefix.
    Djb,
}

impl fmt::Display for KeyType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidArgument(what) => write!(f, "invalid argument: {what}"),
            Error::InvalidState(op, why) => {
                write!(f, "invalid state for call to {op} to succeed: {why}")
            }
            Error::InvalidProtobufEncoding => f.write_str("protobuf encoding was invalid"),
            Error::CiphertextMessageTooShort(n) => {
                write!(f, "ciphertext serialized bytes were too short <{n}>")
            }
            Error::LegacyCiphertextVersion(v) => {
                write!(f, "ciphertext version was too old <{v}>")
            }
            Error::UnrecognizedCiphertextVersion(v) => {
                write!(f, "ciphertext version was unrecognized <{v}>")
            }
            Error::UnrecognizedMessageVersion(v) => {
                write!(f, "unrecognized message version <{v}>")
            }
            Error::NoKeyTypeIdentifier => f.write_str("no key type identifier"),
            Error::BadKeyType(t) => write!(f, "bad key type <{t:#04x}>"),
            Error::BadKeyLength(t, n) => write!(f, "bad key length <{n}> for key with type <{t}>"),
            Error::InvalidKeyAgreement => f.write_str("invalid key agreement"),
            Error::SignatureValidationFailed => f.write_str("invalid signature detected"),
            Error::UntrustedIdentity(_) => f.write_str("untrusted identity"),
            Error::InvalidPreKeyId => f.write_str("invalid prekey identifier"),
            Error::InvalidSignedPreKeyId => f.write_str("invalid signed prekey identifier"),
            Error::InvalidKyberPreKeyId => f.write_str("invalid Kyber prekey identifier"),
            Error::InvalidMacKeyLength(n) => write!(f, "invalid MAC key length <{n}>"),
            Error::NoSenderKeyState => f.write_str("missing sender key state"),
            Error::SessionNotFound(_) => f.write_str("session not found"),
            Error::InvalidSessionStructure(what) => write!(f, "invalid session: {what}"),
            Error::InvalidSenderKeySession => f.write_str("invalid sender key session"),
            Error::InvalidRegistrationId => f.write_str("invalid registration id"),
            Error::DuplicatedMessage(a, b) => write!(f, "message with old counter {a} / {b}"),
            Error::InvalidMessage(t, what) => write!(f, "invalid {t:?} message: {what}"),
            Error::InvalidSealedSenderMessage(what) => {
                write!(f, "invalid sealed sender message: {what}")
            }
            Error::UnknownSealedSenderVersion(v) => {
                write!(f, "unknown sealed sender message version {v}")
            }
            Error::SealedSenderSelfSend => f.write_str("self send of a sealed sender message"),
            Error::UnknownSealedSenderServerCertificateId(id) => {
                write!(f, "unknown server certificate ID {id}")
            }
            Error::BadKemKeyType(t) => write!(f, "bad KEM key type <{t:#04x}>"),
            Error::WrongKemKeyType(t, e) => {
                write!(f, "unexpected KEM key type <{t:#04x}> (expected <{e:#04x}>)")
            }
            Error::BadKemKeyLength(t, n) => {
                write!(f, "bad KEM key length <{n}> for key with type <{t:?}>")
            }
            Error::BadKemCiphertextLength(t, n) => {
                write!(f, "bad KEM ciphertext length <{n}> for key with type <{t:?}>")
            }
            Error::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Store(e) => Some(e),
            _ => None,
        }
    }
}

impl From<StoreError> for Error {
    fn from(e: StoreError) -> Self {
        Error::Store(e)
    }
}
