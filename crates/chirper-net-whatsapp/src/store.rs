//! The Signal protocol store over `LoginCtx` secrets. One secret per record, written after every
//! mutation, so a crash loses at most the message that was being decrypted.
//!
//! Secret names:
//!
//! * `prekey/<id>`: a one-time prekey, removed when a session consumes it
//! * `signed-prekey`: the signed prekey the device registered, id [`keys::SIGNED_PRE_KEY_ID`]
//! * `session/<user>.<device>`: a Double Ratchet session record
//! * `identity-key/<user>`: the identity key first seen for a peer (trust on first use)
//! * `sender-key/<group>/<user>.<device>`: a group sender-key record
//!
//! The local identity key and registration id come from the `device` record, not a secret of their
//! own, so they cannot disagree with the keys the server was registered with.

use chirper_bridge::{Error, LoginCtx, Result};
use chirper_signal_protocol::ProtocolAddress;
use chirper_signal_protocol::curve::{self, PrivateKey, PublicKey};
use chirper_signal_protocol::error::StoreError;
use chirper_signal_protocol::group::{SenderKeyName, SenderKeyRecord};
use chirper_signal_protocol::identity::{IdentityKey, IdentityKeyPair};
use chirper_signal_protocol::prekey::{KyberPreKeyRecord, PreKeyRecord, SignedPreKeyRecord};
use chirper_signal_protocol::session::SessionRecord;
use chirper_signal_protocol::store::{
    IdentityKeyStore, KyberPreKeyStore, PreKeyStore, SenderKeyStore, SessionStore,
    SignedPreKeyStore,
};

use crate::binary::Jid;
use crate::keys::{DeviceKeys, SIGNED_PRE_KEY_ID};

/// The Signal store of one login.
pub struct SignalStore {
    ctx: LoginCtx,
    identity: IdentityKeyPair,
    registration_id: u32,
    signed_pre_key: SignedPreKeyRecord,
}

impl SignalStore {
    /// The store for the device described by `keys`. Writes the signed prekey secret when the
    /// login does not have it yet, so a login that predates the store can still be served.
    pub fn new(ctx: LoginCtx, keys: &DeviceKeys) -> Result<Self> {
        let identity = identity_pair(keys)?;
        let signed_pre_key = signed_pre_key(keys, 0)?;
        let store = Self {
            ctx,
            identity,
            registration_id: keys.registration_id,
            signed_pre_key,
        };
        if store.ctx.get_secret("signed-prekey")?.is_none() {
            store
                .ctx
                .set_secret("signed-prekey", &store.signed_pre_key.serialize())?;
        }
        Ok(store)
    }

    /// The login's secret context, for the code that keeps its own records (app state, history).
    pub fn ctx(&self) -> &LoginCtx {
        &self.ctx
    }

    /// The local identity key, as the server's prekey upload sends it.
    pub fn identity_public(&self) -> [u8; 32] {
        self.identity
            .identity_key()
            .public_key()
            .public_key_bytes()
            .try_into()
            .unwrap_or([0; 32])
    }

    /// The registration id as the server knows it.
    pub fn registration_id(&self) -> u32 {
        self.registration_id
    }

    /// The signed prekey record the device registered.
    pub fn signed_pre_key_record(&self) -> &SignedPreKeyRecord {
        &self.signed_pre_key
    }

    fn load(
        &self,
        operation: &'static str,
        name: &str,
    ) -> std::result::Result<Option<Vec<u8>>, StoreError> {
        self.ctx
            .get_secret(name)
            .map(|value| value.map(|bytes| bytes.to_vec()))
            .map_err(|_| StoreError { operation })
    }

    fn put(
        &self,
        operation: &'static str,
        name: &str,
        bytes: &[u8],
    ) -> std::result::Result<(), StoreError> {
        self.ctx
            .set_secret(name, bytes)
            .map_err(|_| StoreError { operation })
    }

    fn remove(&self, operation: &'static str, name: &str) -> std::result::Result<(), StoreError> {
        self.ctx
            .delete_secret(name)
            .map_err(|_| StoreError { operation })
    }
}

/// The Signal identity key pair of a device.
pub fn identity_pair(keys: &DeviceKeys) -> Result<IdentityKeyPair> {
    let public = PublicKey::from_djb_public_key_bytes(keys.identity.public())
        .map_err(|_| Error::Internal("the identity key is malformed".to_owned()))?;
    let private = PrivateKey::deserialize(keys.identity.private())
        .map_err(|_| Error::Internal("the identity key is malformed".to_owned()))?;
    Ok(IdentityKeyPair::new(IdentityKey::new(public), private))
}

/// The signed prekey of a device, signed by its identity key.
pub fn signed_pre_key(keys: &DeviceKeys, timestamp_ms: u64) -> Result<SignedPreKeyRecord> {
    let malformed = || Error::Internal("the signed prekey is malformed".to_owned());
    let pair = curve::KeyPair::new(
        PublicKey::from_djb_public_key_bytes(keys.signed_pre_key.public())
            .map_err(|_| malformed())?,
        PrivateKey::deserialize(keys.signed_pre_key.private()).map_err(|_| malformed())?,
    );
    Ok(SignedPreKeyRecord::new(
        SIGNED_PRE_KEY_ID,
        timestamp_ms,
        pair,
        keys.signed_pre_key_signature,
    ))
}

/// The protocol address of a device of `user`.
pub fn address(user: &Jid) -> Result<ProtocolAddress> {
    let device = u8::try_from(user.device)
        .map_err(|_| Error::Protocol("a device id is out of range".to_owned()))?;
    Ok(ProtocolAddress::new(user.user.clone(), device))
}

/// The Signal name of a group sender: `(group, sender device)`.
pub fn sender_key_name(group: &Jid, sender: &Jid) -> Result<SenderKeyName> {
    Ok(SenderKeyName::new(group.to_string(), address(sender)?))
}

fn session_secret(address: &ProtocolAddress) -> String {
    format!("session/{}.{}", address.name(), address.device_id())
}

fn identity_secret(address: &ProtocolAddress) -> String {
    format!("identity-key/{}", address.name())
}

fn sender_key_secret(name: &SenderKeyName) -> String {
    format!(
        "sender-key/{}/{}.{}",
        name.group_id(),
        name.sender().name(),
        name.sender().device_id()
    )
}

impl IdentityKeyStore for SignalStore {
    fn identity_key_pair(&self) -> std::result::Result<IdentityKeyPair, StoreError> {
        Ok(self.identity.clone())
    }

    fn local_registration_id(&self) -> std::result::Result<u32, StoreError> {
        Ok(self.registration_id)
    }

    fn save_identity(
        &mut self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
    ) -> std::result::Result<(), StoreError> {
        self.put(
            "save_identity",
            &identity_secret(address),
            &identity.serialize(),
        )
    }

    fn is_trusted_identity(
        &self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
    ) -> std::result::Result<bool, StoreError> {
        match self.get_identity(address)? {
            None => Ok(true),
            Some(known) => Ok(known == *identity),
        }
    }

    fn get_identity(
        &self,
        address: &ProtocolAddress,
    ) -> std::result::Result<Option<IdentityKey>, StoreError> {
        match self.load("get_identity", &identity_secret(address))? {
            None => Ok(None),
            Some(bytes) => IdentityKey::deserialize(&bytes)
                .map(Some)
                .map_err(|_| StoreError {
                    operation: "get_identity",
                }),
        }
    }
}

impl PreKeyStore for SignalStore {
    fn load_pre_key(&self, id: u32) -> std::result::Result<Option<PreKeyRecord>, StoreError> {
        match self.load("load_pre_key", &format!("prekey/{id}"))? {
            None => Ok(None),
            Some(bytes) => PreKeyRecord::deserialize(&bytes)
                .map(Some)
                .map_err(|_| StoreError {
                    operation: "load_pre_key",
                }),
        }
    }

    fn store_pre_key(&mut self, record: &PreKeyRecord) -> std::result::Result<(), StoreError> {
        self.put(
            "store_pre_key",
            &format!("prekey/{}", record.id()),
            &record.serialize(),
        )
    }

    fn remove_pre_key(&mut self, id: u32) -> std::result::Result<(), StoreError> {
        self.remove("remove_pre_key", &format!("prekey/{id}"))
    }
}

impl SignedPreKeyStore for SignalStore {
    fn load_signed_pre_key(
        &self,
        id: u32,
    ) -> std::result::Result<Option<SignedPreKeyRecord>, StoreError> {
        if id != self.signed_pre_key.id() {
            return Ok(None);
        }
        Ok(Some(self.signed_pre_key.clone()))
    }

    fn store_signed_pre_key(
        &mut self,
        record: &SignedPreKeyRecord,
    ) -> std::result::Result<(), StoreError> {
        self.put("store_signed_pre_key", "signed-prekey", &record.serialize())?;
        self.signed_pre_key = record.clone();
        Ok(())
    }
}

impl KyberPreKeyStore for SignalStore {
    /// Post-quantum prekeys are not used by WhatsApp's messages.
    fn load_kyber_pre_key(
        &self,
        _id: u32,
    ) -> std::result::Result<Option<KyberPreKeyRecord>, StoreError> {
        Ok(None)
    }

    fn store_kyber_pre_key(
        &mut self,
        _record: &KyberPreKeyRecord,
    ) -> std::result::Result<(), StoreError> {
        Err(StoreError {
            operation: "store_kyber_pre_key",
        })
    }

    fn mark_kyber_pre_key_used(&mut self, _id: u32) -> std::result::Result<(), StoreError> {
        Ok(())
    }
}

impl SessionStore for SignalStore {
    fn load_session(
        &self,
        address: &ProtocolAddress,
    ) -> std::result::Result<Option<SessionRecord>, StoreError> {
        match self.load("load_session", &session_secret(address))? {
            None => Ok(None),
            Some(bytes) => SessionRecord::deserialize(&bytes)
                .map(Some)
                .map_err(|_| StoreError {
                    operation: "load_session",
                }),
        }
    }

    fn store_session(
        &mut self,
        address: &ProtocolAddress,
        record: &SessionRecord,
    ) -> std::result::Result<(), StoreError> {
        self.put(
            "store_session",
            &session_secret(address),
            &record.serialize(),
        )
    }
}

impl SenderKeyStore for SignalStore {
    fn load_sender_key(
        &self,
        name: &SenderKeyName,
    ) -> std::result::Result<Option<SenderKeyRecord>, StoreError> {
        match self.load("load_sender_key", &sender_key_secret(name))? {
            None => Ok(None),
            Some(bytes) => SenderKeyRecord::deserialize(&bytes)
                .map(Some)
                .map_err(|_| StoreError {
                    operation: "load_sender_key",
                }),
        }
    }

    fn store_sender_key(
        &mut self,
        name: &SenderKeyName,
        record: &SenderKeyRecord,
    ) -> std::result::Result<(), StoreError> {
        self.put(
            "store_sender_key",
            &sender_key_secret(name),
            &record.serialize(),
        )
    }
}
