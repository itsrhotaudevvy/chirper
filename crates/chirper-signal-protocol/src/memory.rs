//! An in-memory implementation of every store trait, for tests and simple drivers.
//!
//! Trust is trust-on-first-use: the first identity key seen for an address is trusted, and a
//! different key for that address is rejected until the driver replaces it deliberately.

use std::collections::HashMap;
use std::fmt;

use crate::ProtocolAddress;
use crate::error::StoreError;
use crate::group::{SenderKeyName, SenderKeyRecord};
use crate::identity::{IdentityKey, IdentityKeyPair};
use crate::prekey::{KyberPreKeyRecord, PreKeyRecord, SignedPreKeyRecord};
use crate::session::SessionRecord;
use crate::store::{
    IdentityKeyStore, KyberPreKeyStore, PreKeyStore, SenderKeyStore, SessionStore,
    SignedPreKeyStore,
};

/// All stores in one value, held in process memory.
pub struct InMemoryProtocolStore {
    identity: IdentityKeyPair,
    registration_id: u32,
    identities: HashMap<ProtocolAddress, IdentityKey>,
    pre_keys: HashMap<u32, PreKeyRecord>,
    signed_pre_keys: HashMap<u32, SignedPreKeyRecord>,
    kyber_pre_keys: HashMap<u32, KyberPreKeyRecord>,
    sessions: HashMap<ProtocolAddress, SessionRecord>,
    sender_keys: HashMap<SenderKeyName, SenderKeyRecord>,
}

impl InMemoryProtocolStore {
    /// An empty store for a device with the given identity and registration id.
    pub fn new(identity: IdentityKeyPair, registration_id: u32) -> Self {
        Self {
            identity,
            registration_id,
            identities: HashMap::new(),
            pre_keys: HashMap::new(),
            signed_pre_keys: HashMap::new(),
            kyber_pre_keys: HashMap::new(),
            sessions: HashMap::new(),
            sender_keys: HashMap::new(),
        }
    }

    /// Whether a one-time prekey with `id` is stored.
    pub fn contains_pre_key(&self, id: u32) -> bool {
        self.pre_keys.contains_key(&id)
    }

    /// Whether a Kyber prekey with `id` is stored.
    pub fn contains_kyber_pre_key(&self, id: u32) -> bool {
        self.kyber_pre_keys.contains_key(&id)
    }
}

impl fmt::Debug for InMemoryProtocolStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("InMemoryProtocolStore [redacted]")
    }
}

impl IdentityKeyStore for InMemoryProtocolStore {
    fn identity_key_pair(&self) -> Result<IdentityKeyPair, StoreError> {
        Ok(self.identity.clone())
    }

    fn local_registration_id(&self) -> Result<u32, StoreError> {
        Ok(self.registration_id)
    }

    fn save_identity(
        &mut self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
    ) -> Result<(), StoreError> {
        self.identities.insert(address.clone(), identity.clone());
        Ok(())
    }

    fn is_trusted_identity(
        &self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
    ) -> Result<bool, StoreError> {
        Ok(self
            .identities
            .get(address)
            .is_none_or(|known| known == identity))
    }

    fn get_identity(&self, address: &ProtocolAddress) -> Result<Option<IdentityKey>, StoreError> {
        Ok(self.identities.get(address).cloned())
    }
}

impl PreKeyStore for InMemoryProtocolStore {
    fn load_pre_key(&self, id: u32) -> Result<Option<PreKeyRecord>, StoreError> {
        Ok(self.pre_keys.get(&id).cloned())
    }

    fn store_pre_key(&mut self, record: &PreKeyRecord) -> Result<(), StoreError> {
        self.pre_keys.insert(record.id(), record.clone());
        Ok(())
    }

    fn remove_pre_key(&mut self, id: u32) -> Result<(), StoreError> {
        self.pre_keys.remove(&id);
        Ok(())
    }
}

impl SignedPreKeyStore for InMemoryProtocolStore {
    fn load_signed_pre_key(&self, id: u32) -> Result<Option<SignedPreKeyRecord>, StoreError> {
        Ok(self.signed_pre_keys.get(&id).cloned())
    }

    fn store_signed_pre_key(&mut self, record: &SignedPreKeyRecord) -> Result<(), StoreError> {
        self.signed_pre_keys.insert(record.id(), record.clone());
        Ok(())
    }
}

impl KyberPreKeyStore for InMemoryProtocolStore {
    fn load_kyber_pre_key(&self, id: u32) -> Result<Option<KyberPreKeyRecord>, StoreError> {
        Ok(self.kyber_pre_keys.get(&id).cloned())
    }

    fn store_kyber_pre_key(&mut self, record: &KyberPreKeyRecord) -> Result<(), StoreError> {
        self.kyber_pre_keys.insert(record.id(), record.clone());
        Ok(())
    }

    fn mark_kyber_pre_key_used(&mut self, id: u32) -> Result<(), StoreError> {
        self.kyber_pre_keys.remove(&id);
        Ok(())
    }
}

impl SessionStore for InMemoryProtocolStore {
    fn load_session(&self, address: &ProtocolAddress) -> Result<Option<SessionRecord>, StoreError> {
        Ok(self.sessions.get(address).cloned())
    }

    fn store_session(
        &mut self,
        address: &ProtocolAddress,
        record: &SessionRecord,
    ) -> Result<(), StoreError> {
        self.sessions.insert(address.clone(), record.clone());
        Ok(())
    }
}

impl SenderKeyStore for InMemoryProtocolStore {
    fn load_sender_key(&self, name: &SenderKeyName) -> Result<Option<SenderKeyRecord>, StoreError> {
        Ok(self.sender_keys.get(name).cloned())
    }

    fn store_sender_key(
        &mut self,
        name: &SenderKeyName,
        record: &SenderKeyRecord,
    ) -> Result<(), StoreError> {
        self.sender_keys.insert(name.clone(), record.clone());
        Ok(())
    }
}
