//! Storage traits that the driver implements. They are synchronous and storage-agnostic.
//!
//! Loads take `&self`; stores and removals take `&mut self`. A driver that persists records
//! should write the output of each record's `serialize` and read it back with `deserialize`.
//! Every method returns [`StoreError`] when the backend fails; the crate never inspects it.

use crate::ProtocolAddress;
use crate::error::StoreError;
use crate::group::{SenderKeyName, SenderKeyRecord};
use crate::identity::{IdentityKey, IdentityKeyPair};
use crate::prekey::{KyberPreKeyRecord, PreKeyRecord, SignedPreKeyRecord};
use crate::session::SessionRecord;

/// Our own identity and the trust decisions made about remote identities.
pub trait IdentityKeyStore {
    /// Our identity key pair.
    fn identity_key_pair(&self) -> Result<IdentityKeyPair, StoreError>;

    /// Our registration id.
    fn local_registration_id(&self) -> Result<u32, StoreError>;

    /// Records the identity key seen for `address`.
    fn save_identity(
        &mut self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
    ) -> Result<(), StoreError>;

    /// Whether `identity` may be used for `address`. Implementations decide the policy.
    fn is_trusted_identity(
        &self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
    ) -> Result<bool, StoreError>;

    /// The identity key stored for `address`, if any.
    fn get_identity(&self, address: &ProtocolAddress) -> Result<Option<IdentityKey>, StoreError>;
}

/// One-time prekeys.
pub trait PreKeyStore {
    /// The one-time prekey with `id`, if it exists.
    fn load_pre_key(&self, id: u32) -> Result<Option<PreKeyRecord>, StoreError>;

    /// Stores a one-time prekey under its id.
    fn store_pre_key(&mut self, record: &PreKeyRecord) -> Result<(), StoreError>;

    /// Deletes the one-time prekey with `id` after it has been used.
    fn remove_pre_key(&mut self, id: u32) -> Result<(), StoreError>;
}

/// Signed prekeys.
pub trait SignedPreKeyStore {
    /// The signed prekey with `id`, if it exists.
    fn load_signed_pre_key(&self, id: u32) -> Result<Option<SignedPreKeyRecord>, StoreError>;

    /// Stores a signed prekey under its id.
    fn store_signed_pre_key(&mut self, record: &SignedPreKeyRecord) -> Result<(), StoreError>;
}

/// Kyber prekeys (PQXDH).
pub trait KyberPreKeyStore {
    /// The Kyber prekey with `id`, if it exists.
    fn load_kyber_pre_key(&self, id: u32) -> Result<Option<KyberPreKeyRecord>, StoreError>;

    /// Stores a Kyber prekey under its id.
    fn store_kyber_pre_key(&mut self, record: &KyberPreKeyRecord) -> Result<(), StoreError>;

    /// Records that the Kyber prekey `id` was used by an accepted session. A one-time Kyber
    /// prekey should then stop being offered; a last-resort key may be kept.
    fn mark_kyber_pre_key_used(&mut self, id: u32) -> Result<(), StoreError>;
}

/// Double Ratchet session records, one per peer address.
pub trait SessionStore {
    /// The record for `address`, or `None` if there has never been a session.
    fn load_session(&self, address: &ProtocolAddress) -> Result<Option<SessionRecord>, StoreError>;

    /// Stores the record for `address`, replacing any previous record.
    fn store_session(
        &mut self,
        address: &ProtocolAddress,
        record: &SessionRecord,
    ) -> Result<(), StoreError>;
}

/// Sender key records, one per (group, sender) pair.
pub trait SenderKeyStore {
    /// The sender key record for `name`, if any.
    fn load_sender_key(&self, name: &SenderKeyName) -> Result<Option<SenderKeyRecord>, StoreError>;

    /// Stores the sender key record for `name`, replacing any previous record.
    fn store_sender_key(
        &mut self,
        name: &SenderKeyName,
        record: &SenderKeyRecord,
    ) -> Result<(), StoreError>;
}

/// Everything a one-to-one session needs. Implemented automatically for any type that
/// implements the five one-to-one store traits.
pub trait ProtocolStore:
    IdentityKeyStore + PreKeyStore + SignedPreKeyStore + KyberPreKeyStore + SessionStore
{
}

impl<T> ProtocolStore for T where
    T: IdentityKeyStore
        + PreKeyStore
        + SignedPreKeyStore
        + KyberPreKeyStore
        + SessionStore
        + ?Sized
{
}
