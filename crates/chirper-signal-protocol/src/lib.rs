//! Chirper's own implementation of the Signal protocol, shared by the WhatsApp, Signal and
//! Messenger drivers.
//!
//! This crate is a pure, synchronous, storage-agnostic port of the parts of libsignal
//! (v0.106.1, AGPL-3.0) that the drivers need: curve keys and XEdDSA, identity keys, X3DH and
//! PQXDH key agreement, the Double Ratchet, wire messages, sender keys and sealed sender.
//! Storage is supplied by the driver through the traits in [`store`].

#![forbid(unsafe_code)]

pub mod curve;
pub mod error;
pub mod kem;
pub mod protocol;

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
