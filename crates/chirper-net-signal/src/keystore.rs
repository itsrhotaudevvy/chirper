//! The driver's persistent protocol store: identities, prekeys, sessions and sender keys.
//!
//! The protocol crate defines storage traits and no backend. This type implements them over
//! ordered maps of serialized records and round-trips the whole thing through one JSON secret
//! (`store`). Each record is written with its own `serialize` and read with `deserialize`, as the
//! protocol crate's documentation asks.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use chirper_signal_protocol::group::{SenderKeyName, SenderKeyRecord};
use chirper_signal_protocol::identity::{IdentityKey, IdentityKeyPair};
use chirper_signal_protocol::prekey::{KyberPreKeyRecord, PreKeyRecord, SignedPreKeyRecord};
use chirper_signal_protocol::session::SessionRecord;
use chirper_signal_protocol::store::{
    IdentityKeyStore, KyberPreKeyStore, PreKeyStore, SenderKeyStore, SessionStore,
    SignedPreKeyStore,
};
use chirper_signal_protocol::{ProtocolAddress, StoreError};

use crate::account::b64;

/// The serialized form of [`KeyStore`].
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct KeyStore {
    #[serde(with = "b64")]
    identity: Vec<u8>,
    registration_id: u32,
    #[serde(default)]
    pre_keys: BTreeMap<u32, String>,
    #[serde(default)]
    signed_pre_keys: BTreeMap<u32, String>,
    #[serde(default)]
    kyber_pre_keys: BTreeMap<u32, String>,
    /// Kyber prekeys that are last-resort: they are never removed when used.
    #[serde(default)]
    last_resort_kyber: BTreeSet<u32>,
    #[serde(default)]
    sessions: BTreeMap<String, String>,
    #[serde(default)]
    identities: BTreeMap<String, String>,
    #[serde(default)]
    sender_keys: BTreeMap<String, String>,
    /// Registration id of each remote device, by address.
    #[serde(default)]
    registration_ids: BTreeMap<String, u32>,
    /// Profile key of each remote account (base64), learned from messages and profiles.
    #[serde(default)]
    profile_keys: BTreeMap<String, String>,
}

impl std::fmt::Debug for KeyStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("KeyStore [redacted]")
    }
}

/// The store key of a protocol address.
fn address_key(address: &ProtocolAddress) -> String {
    format!("{}.{}", address.name(), address.device_id())
}

/// The store key of a sender key: group, then sender.
fn sender_key_name(name: &SenderKeyName) -> String {
    format!(
        "{}|{}",
        name.group_id(),
        address_key(name.sender())
    )
}

fn encode(bytes: &[u8]) -> String {
    b64::encode(bytes)
}

fn decode(text: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(text).ok()
}

fn load<T>(
    map: &BTreeMap<String, String>,
    key: &str,
    operation: &'static str,
    parse: impl FnOnce(&[u8]) -> Option<T>,
) -> Result<Option<T>, StoreError> {
    match map.get(key) {
        None => Ok(None),
        Some(text) => decode(text)
            .as_deref()
            .and_then(parse)
            .map(Some)
            .ok_or(StoreError { operation }),
    }
}

impl KeyStore {
    /// A store for a freshly linked device.
    pub fn new(identity: &IdentityKeyPair, registration_id: u32) -> Self {
        Self {
            identity: identity.serialize().to_vec(),
            registration_id,
            ..Self::default()
        }
    }

    /// The JSON form, written to the `store` secret.
    pub fn to_json(&self) -> serde_json::Result<Vec<u8>> {
        serde_json::to_vec(self)
    }

    /// Reads the JSON form.
    pub fn from_json(bytes: &[u8]) -> serde_json::Result<Self> {
        serde_json::from_slice(bytes)
    }

    /// Number of one-time EC prekeys held.
    pub fn pre_key_count(&self) -> usize {
        self.pre_keys.len()
    }

    /// Number of one-time Kyber prekeys held (last-resort keys excluded).
    pub fn kyber_pre_key_count(&self) -> usize {
        self.kyber_pre_keys
            .keys()
            .filter(|id| !self.last_resort_kyber.contains(id))
            .count()
    }

    /// The largest one-time prekey id held, or 0.
    pub fn max_pre_key_id(&self) -> u32 {
        self.pre_keys.keys().next_back().copied().unwrap_or(0)
    }

    /// The largest one-time Kyber prekey id held, or 0.
    pub fn max_kyber_pre_key_id(&self) -> u32 {
        self.kyber_pre_keys.keys().next_back().copied().unwrap_or(0)
    }

    /// Marks a Kyber prekey as last-resort, so using it does not remove it.
    pub fn mark_last_resort(&mut self, id: u32) {
        self.last_resort_kyber.insert(id);
    }

    /// The ids of all one-time EC prekeys, oldest first.
    pub fn pre_key_ids(&self) -> Vec<u32> {
        self.pre_keys.keys().copied().collect()
    }

    /// The one-time EC prekeys held, as records.
    pub fn pre_key_records(&self) -> Vec<PreKeyRecord> {
        self.pre_keys
            .values()
            .filter_map(|text| decode(text))
            .filter_map(|bytes| PreKeyRecord::deserialize(&bytes).ok())
            .collect()
    }

    /// The one-time Kyber prekeys held (last-resort keys excluded), as records.
    pub fn kyber_records(&self) -> Vec<KyberPreKeyRecord> {
        self.kyber_pre_keys
            .iter()
            .filter(|(id, _)| !self.last_resort_kyber.contains(id))
            .filter_map(|(_, text)| decode(text))
            .filter_map(|bytes| KyberPreKeyRecord::deserialize(&bytes).ok())
            .collect()
    }

    /// The signed prekey with the given id, if held.
    pub fn signed_record(&self, id: u32) -> Option<SignedPreKeyRecord> {
        self.signed_pre_keys
            .get(&id)
            .and_then(|text| decode(text))
            .and_then(|bytes| SignedPreKeyRecord::deserialize(&bytes).ok())
    }

    /// The identity key pair.
    pub fn identity(&self) -> Option<IdentityKeyPair> {
        IdentityKeyPair::deserialize(&self.identity).ok()
    }

    /// The device ids of `aci` that have a session with us, ascending.
    pub fn devices_with_sessions(&self, aci: &str) -> Vec<u32> {
        let prefix = format!("{aci}.");
        let mut devices: Vec<u32> = self
            .sessions
            .keys()
            .filter_map(|key| key.strip_prefix(&prefix))
            .filter_map(|device| device.parse().ok())
            .collect();
        devices.sort_unstable();
        devices
    }

    /// Records the registration id a remote device announced in its prekey bundle.
    pub fn set_registration_id(&mut self, address: &ProtocolAddress, id: u32) {
        self.registration_ids.insert(address_key(address), id);
    }

    /// The registration id of a remote device, if known.
    pub fn registration_id(&self, address: &ProtocolAddress) -> Option<u32> {
        self.registration_ids.get(&address_key(address)).copied()
    }

    /// Stores the profile key of a remote account (32 bytes).
    pub fn set_profile_key(&mut self, aci: &str, key: &[u8]) {
        self.profile_keys.insert(aci.to_owned(), encode(key));
    }

    /// The profile key of a remote account, if known.
    pub fn profile_key(&self, aci: &str) -> Option<Vec<u8>> {
        self.profile_keys.get(aci).and_then(|text| decode(text))
    }

    /// Drops the session with one device, so the next message starts a new one.
    pub fn remove_session(&mut self, address: &ProtocolAddress) {
        self.sessions.remove(&address_key(address));
    }

    /// Drops every one-time prekey and Kyber prekey (used when rebuilding keys).
    pub fn clear_one_time_keys(&mut self) {
        self.pre_keys.clear();
        self.kyber_pre_keys.retain(|id, _| self.last_resort_kyber.contains(id));
    }
}

impl IdentityKeyStore for KeyStore {
    fn identity_key_pair(&self) -> Result<IdentityKeyPair, StoreError> {
        IdentityKeyPair::deserialize(&self.identity).map_err(|_| StoreError {
            operation: "identity_key_pair",
        })
    }

    fn local_registration_id(&self) -> Result<u32, StoreError> {
        Ok(self.registration_id)
    }

    fn save_identity(
        &mut self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
    ) -> Result<(), StoreError> {
        self.identities
            .insert(address_key(address), encode(&identity.serialize()));
        Ok(())
    }

    fn is_trusted_identity(
        &self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
    ) -> Result<bool, StoreError> {
        // Trust on first use, as libsignal's default store does.
        Ok(self
            .get_identity(address)?
            .is_none_or(|known| known == *identity))
    }

    fn get_identity(&self, address: &ProtocolAddress) -> Result<Option<IdentityKey>, StoreError> {
        load(
            &self.identities,
            &address_key(address),
            "get_identity",
            |bytes| IdentityKey::deserialize(bytes).ok(),
        )
    }
}

impl PreKeyStore for KeyStore {
    fn load_pre_key(&self, id: u32) -> Result<Option<PreKeyRecord>, StoreError> {
        match self.pre_keys.get(&id) {
            None => Ok(None),
            Some(text) => decode(text)
                .and_then(|bytes| PreKeyRecord::deserialize(&bytes).ok())
                .map(Some)
                .ok_or(StoreError {
                    operation: "load_pre_key",
                }),
        }
    }

    fn store_pre_key(&mut self, record: &PreKeyRecord) -> Result<(), StoreError> {
        self.pre_keys
            .insert(record.id(), encode(&record.serialize()));
        Ok(())
    }

    fn remove_pre_key(&mut self, id: u32) -> Result<(), StoreError> {
        self.pre_keys.remove(&id);
        Ok(())
    }
}

impl SignedPreKeyStore for KeyStore {
    fn load_signed_pre_key(&self, id: u32) -> Result<Option<SignedPreKeyRecord>, StoreError> {
        match self.signed_pre_keys.get(&id) {
            None => Ok(None),
            Some(text) => decode(text)
                .and_then(|bytes| SignedPreKeyRecord::deserialize(&bytes).ok())
                .map(Some)
                .ok_or(StoreError {
                    operation: "load_signed_pre_key",
                }),
        }
    }

    fn store_signed_pre_key(&mut self, record: &SignedPreKeyRecord) -> Result<(), StoreError> {
        self.signed_pre_keys
            .insert(record.id(), encode(&record.serialize()));
        Ok(())
    }
}

impl KyberPreKeyStore for KeyStore {
    fn load_kyber_pre_key(&self, id: u32) -> Result<Option<KyberPreKeyRecord>, StoreError> {
        match self.kyber_pre_keys.get(&id) {
            None => Ok(None),
            Some(text) => decode(text)
                .and_then(|bytes| KyberPreKeyRecord::deserialize(&bytes).ok())
                .map(Some)
                .ok_or(StoreError {
                    operation: "load_kyber_pre_key",
                }),
        }
    }

    fn store_kyber_pre_key(&mut self, record: &KyberPreKeyRecord) -> Result<(), StoreError> {
        self.kyber_pre_keys
            .insert(record.id(), encode(&record.serialize()));
        Ok(())
    }

    fn mark_kyber_pre_key_used(&mut self, id: u32) -> Result<(), StoreError> {
        if !self.last_resort_kyber.contains(&id) {
            self.kyber_pre_keys.remove(&id);
        }
        Ok(())
    }
}

impl SessionStore for KeyStore {
    fn load_session(&self, address: &ProtocolAddress) -> Result<Option<SessionRecord>, StoreError> {
        load(&self.sessions, &address_key(address), "load_session", |bytes| {
            SessionRecord::deserialize(bytes).ok()
        })
    }

    fn store_session(
        &mut self,
        address: &ProtocolAddress,
        record: &SessionRecord,
    ) -> Result<(), StoreError> {
        self.sessions
            .insert(address_key(address), encode(&record.serialize()));
        Ok(())
    }
}

impl SenderKeyStore for KeyStore {
    fn load_sender_key(&self, name: &SenderKeyName) -> Result<Option<SenderKeyRecord>, StoreError> {
        load(&self.sender_keys, &sender_key_name(name), "load_sender_key", |bytes| {
            SenderKeyRecord::deserialize(bytes).ok()
        })
    }

    fn store_sender_key(
        &mut self,
        name: &SenderKeyName,
        record: &SenderKeyRecord,
    ) -> Result<(), StoreError> {
        self.sender_keys
            .insert(sender_key_name(name), encode(&record.serialize()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chirper_signal_protocol::prekey::{generate_kyber_pre_key, generate_pre_keys, generate_signed_pre_key};
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    #[test]
    fn prekeys_survive_a_json_round_trip() {
        let mut rng = StdRng::seed_from_u64(7);
        let identity = IdentityKeyPair::generate(&mut rng);
        let mut store = KeyStore::new(&identity, 4242);
        for record in generate_pre_keys(1, 3, &mut rng).expect("prekeys") {
            store.store_pre_key(&record).expect("store");
        }
        let signed = generate_signed_pre_key(&identity, 1, 1_700_000_000_000, &mut rng).expect("signed");
        store.store_signed_pre_key(&signed).expect("store signed");
        let kyber = generate_kyber_pre_key(&identity, 1, 1_700_000_000_000, &mut rng).expect("kyber");
        store.store_kyber_pre_key(&kyber).expect("store kyber");
        store.mark_last_resort(1);

        let bytes = store.to_json().expect("json");
        let back = KeyStore::from_json(&bytes).expect("parse");
        assert_eq!(back.pre_key_count(), 3);
        assert_eq!(back.kyber_pre_key_count(), 0);
        assert_eq!(back.local_registration_id().expect("id"), 4242);
        assert!(back.load_pre_key(2).expect("load").is_some());
        assert!(back.load_signed_pre_key(1).expect("load").is_some());
        assert_eq!(
            back.identity_key_pair().expect("identity").identity_key(),
            identity.identity_key()
        );
    }

    #[test]
    fn a_used_one_time_kyber_key_is_removed_but_last_resort_is_kept() {
        let mut rng = StdRng::seed_from_u64(9);
        let identity = IdentityKeyPair::generate(&mut rng);
        let mut store = KeyStore::new(&identity, 1);
        let one_time = generate_kyber_pre_key(&identity, 5, 0, &mut rng).expect("kyber");
        let last_resort = generate_kyber_pre_key(&identity, 6, 0, &mut rng).expect("kyber");
        store.store_kyber_pre_key(&one_time).expect("store");
        store.store_kyber_pre_key(&last_resort).expect("store");
        store.mark_last_resort(6);
        store.mark_kyber_pre_key_used(5).expect("mark");
        store.mark_kyber_pre_key_used(6).expect("mark");
        assert!(store.load_kyber_pre_key(5).expect("load").is_none());
        assert!(store.load_kyber_pre_key(6).expect("load").is_some());
    }
}
