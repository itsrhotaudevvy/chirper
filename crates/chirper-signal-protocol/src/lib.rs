//! Chirper's own implementation of the Signal protocol, shared by the WhatsApp, Signal and
//! Messenger drivers. It is a pure, synchronous, storage-agnostic port of the parts of libsignal
//! that the drivers need. No libsignal crate is used.
//!
//! Implemented:
//!
//! * Curve25519 keys with XEdDSA signatures ([`curve`]), and Kyber1024 / ML-KEM-1024 KEMs
//!   through `libcrux-ml-kem` ([`kem`]).
//! * Identity keys and key pairs ([`identity`]), one-time, signed and Kyber prekeys, and the
//!   prekey bundle ([`prekey`]).
//! * HKDF-SHA256, the root, chain and message key steps ([`kdf`]).
//! * X3DH (session version 3) and PQXDH (session version 4) for both roles ([`ratchet`]).
//! * The Double Ratchet session state and records, with 40 archived states, 5 receiving
//!   chains, 2000 saved message keys per chain and a 25 000-message forward-jump cap
//!   ([`session`]).
//! * Wire messages: `SignalMessage`, `PreKeySignalMessage`, `SenderKeyMessage` and
//!   `SenderKeyDistributionMessage`, with strict parsing and MAC/signature checks ([`message`]).
//! * Session establishment from a bundle and from a prekey message ([`session_builder`]), and
//!   one-to-one encryption and transactional decryption ([`cipher`]).
//! * Sender keys for group messages ([`group`]).
//! * Sealed sender v1: encrypt, decrypt, server and sender certificates, and certificate
//!   validation against a trust root ([`sealed_sender`]).
//! * The storage traits ([`store`]) and an in-memory implementation of all of them ([`memory`]).
//!
//! Not implemented: sealed sender v2 (multi-recipient), the SPQR triple ratchet, the legacy
//! v2 key exchange, fingerprints, device provisioning, and sealed sender certificates that name
//! their signer by id only (these are rejected, not accepted unchecked).
//!
//! Storage is supplied by the driver through the traits in [`store`].

#![forbid(unsafe_code)]

pub mod cipher;
pub mod curve;
pub mod error;
pub mod group;
pub mod identity;
pub mod kdf;
pub mod kem;
pub mod memory;
pub mod message;
pub mod prekey;
pub mod protocol;
pub mod ratchet;
pub mod sealed_sender;
pub mod session;
pub mod session_builder;
pub mod store;

mod proto;

pub use curve::{KeyPair, PrivateKey, PublicKey};
pub use error::{Error, KeyType, Result, StoreError};

/// A remote (or local) protocol address: an account name plus a device id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProtocolAddress {
    name: String,
    device_id: u8,
}

impl ProtocolAddress {
    /// Creates an address from an account name and a device id.
    pub fn new(name: impl Into<String>, device_id: u8) -> Self {
        Self {
            name: name.into(),
            device_id,
        }
    }

    /// The account name. Never logged by this crate.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The device id.
    pub fn device_id(&self) -> u8 {
        self.device_id
    }
}
