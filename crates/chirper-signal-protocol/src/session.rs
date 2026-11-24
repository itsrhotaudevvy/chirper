//! Session state and session records: the Double Ratchet state of one (address) peer.
//!
//! Port of libsignal `state/record/{SessionRecord,SessionState,ChainState}.go` and
//! `session/SessionCipher.go` limits (AGPL-3.0-only, Signal Messenger LLC). A record holds the
//! current state plus up to [`MAX_ARCHIVED_STATES`] archived states. A state holds one sending
//! chain, up to [`MAX_RECEIVER_CHAINS`] receiving chains, and up to [`MAX_MESSAGE_KEYS`] saved
//! message keys per receiving chain. Records serialize to `RecordStructure`.

use std::collections::VecDeque;
use std::fmt;

use prost::Message;
use zeroize::Zeroizing;

use crate::curve::{KeyPair, PublicKey};
use crate::error::{Error, Result};
use crate::identity::IdentityKey;
use crate::kdf::{ChainKey, MessageKeys, RootKey};
use crate::proto::{
    self, Chain, MessageKey, PendingKyberPreKey, PendingPreKey, RecordStructure, SessionStructure,
};

/// Maximum number of archived states kept in a record.
pub const MAX_ARCHIVED_STATES: usize = 40;
/// Maximum number of receiving chains kept in a state; the oldest is dropped first.
pub const MAX_RECEIVER_CHAINS: usize = 5;
/// Maximum number of skipped message keys kept per receiving chain.
pub const MAX_MESSAGE_KEYS: usize = 2000;
/// Largest counter gap a message may jump forward before it is rejected.
pub const MAX_FORWARD_JUMPS: u32 = 25_000;

/// The sending side of a ratchet: our ratchet key pair and the current chain key.
#[derive(Debug, Clone)]
pub struct SenderChain {
    /// Our current ratchet key pair (the private half is zeroized on drop).
    pub ratchet_key: KeyPair,
    /// The chain key for the next message to send.
    pub chain_key: ChainKey,
}

/// A receiving chain, keyed by the peer's ratchet public key.
#[derive(Debug, Clone)]
pub struct ReceiverChain {
    /// The peer's ratchet public key that this chain belongs to.
    pub sender_ratchet_key: PublicKey,
    /// The chain key for the next message expected on this chain.
    pub chain_key: ChainKey,
    /// Keys for skipped messages, oldest first, so they can still be decrypted.
    pub message_keys: VecDeque<MessageKeys>,
}

/// A prekey message we sent that the peer has not yet acknowledged (initiator side).
#[derive(Debug, Clone)]
pub struct PendingPreKeyMessage {
    /// One-time prekey id the message used, if any.
    pub pre_key_id: Option<u32>,
    /// Signed prekey id the message used.
    pub signed_pre_key_id: u32,
    /// Our base key sent in the message.
    pub base_key: PublicKey,
}

/// A Kyber prekey message we sent that the peer has not yet acknowledged (PQXDH).
#[derive(Debug, Clone)]
pub struct PendingKyberMessage {
    /// Kyber prekey id the message used.
    pub kyber_pre_key_id: u32,
    /// KEM ciphertext to resend until acknowledged.
    pub ciphertext: Vec<u8>,
}

/// One Double Ratchet session state.
#[derive(Clone)]
pub struct SessionState {
    pub(crate) version: u32,
    pub(crate) local_identity: IdentityKey,
    pub(crate) remote_identity: IdentityKey,
    pub(crate) root_key: RootKey,
    pub(crate) previous_counter: u32,
    pub(crate) sender_chain: Option<SenderChain>,
    pub(crate) receiver_chains: Vec<ReceiverChain>,
    pub(crate) pending_pre_key: Option<PendingPreKeyMessage>,
    pub(crate) pending_kyber: Option<PendingKyberMessage>,
    pub(crate) local_registration_id: u32,
    pub(crate) remote_registration_id: u32,
    pub(crate) alice_base_key: Option<PublicKey>,
}

impl SessionState {
    /// Creates a state with no chains yet.
    pub fn new(
        version: u32,
        local_identity: IdentityKey,
        remote_identity: IdentityKey,
        root_key: RootKey,
        local_registration_id: u32,
        remote_registration_id: u32,
    ) -> Self {
        Self {
            version,
            local_identity,
            remote_identity,
            root_key,
            previous_counter: 0,
            sender_chain: None,
            receiver_chains: Vec::new(),
            pending_pre_key: None,
            pending_kyber: None,
            local_registration_id,
            remote_registration_id,
            alice_base_key: None,
        }
    }

    /// The message version of this state (3 or 4).
    pub fn version(&self) -> u32 {
        self.version
    }

    /// The peer's identity key.
    pub fn remote_identity(&self) -> &IdentityKey {
        &self.remote_identity
    }

    /// Our identity key.
    pub fn local_identity(&self) -> &IdentityKey {
        &self.local_identity
    }

    /// The sending chain, if the state can send.
    pub fn sender_chain(&self) -> Option<&SenderChain> {
        self.sender_chain.as_ref()
    }

    /// The prekey message awaiting acknowledgement, if any.
    pub fn pending_pre_key(&self) -> Option<&PendingPreKeyMessage> {
        self.pending_pre_key.as_ref()
    }

    /// Whether `base_key` is this state's initiator base key (used to detect duplicates).
    pub(crate) fn has_alice_base_key(&self, base_key: &PublicKey) -> bool {
        self.alice_base_key.as_ref() == Some(base_key)
    }

    /// Records a new sending chain.
    pub(crate) fn set_sender_chain(&mut self, ratchet_key: KeyPair, chain_key: ChainKey) {
        self.sender_chain = Some(SenderChain {
            ratchet_key,
            chain_key,
        });
    }

    /// Adds a receiving chain, dropping the oldest if there are too many.
    pub(crate) fn add_receiver_chain(
        &mut self,
        sender_ratchet_key: PublicKey,
        chain_key: ChainKey,
    ) {
        self.receiver_chains.push(ReceiverChain {
            sender_ratchet_key,
            chain_key,
            message_keys: VecDeque::new(),
        });
        if self.receiver_chains.len() > MAX_RECEIVER_CHAINS {
            self.receiver_chains.remove(0);
        }
    }

    /// The index of the receiving chain for `sender_ratchet_key`, if we have one.
    pub(crate) fn receiver_chain_index(&self, sender_ratchet_key: &PublicKey) -> Option<usize> {
        self.receiver_chains
            .iter()
            .position(|chain| &chain.sender_ratchet_key == sender_ratchet_key)
    }

    /// Saves the keys of a skipped message, dropping the oldest beyond [`MAX_MESSAGE_KEYS`].
    pub(crate) fn save_message_keys(&mut self, index: usize, keys: MessageKeys) {
        if let Some(chain) = self.receiver_chains.get_mut(index) {
            chain.message_keys.push_back(keys);
            while chain.message_keys.len() > MAX_MESSAGE_KEYS {
                chain.message_keys.pop_front();
            }
        }
    }

    /// Removes and returns the saved keys for `counter` on receiving chain `index`.
    pub(crate) fn take_message_keys(&mut self, index: usize, counter: u32) -> Option<MessageKeys> {
        let chain = self.receiver_chains.get_mut(index)?;
        let position = chain
            .message_keys
            .iter()
            .position(|k| k.index() == counter)?;
        chain.message_keys.remove(position)
    }

    /// Clears the pending prekey and Kyber bookkeeping once the peer has replied.
    pub(crate) fn clear_pending(&mut self) {
        self.pending_pre_key = None;
        self.pending_kyber = None;
    }

    /// Sets the pending prekey message (initiator side).
    pub(crate) fn set_pending_pre_key(&mut self, pending: PendingPreKeyMessage) {
        self.pending_pre_key = Some(pending);
    }

    /// Sets the pending Kyber message (PQXDH initiator side).
    pub(crate) fn set_pending_kyber(&mut self, pending: PendingKyberMessage) {
        self.pending_kyber = Some(pending);
    }

    /// Converts to the protobuf storage form.
    pub(crate) fn to_structure(&self) -> SessionStructure {
        SessionStructure {
            session_version: Some(self.version),
            local_identity_public: Some(self.local_identity.serialize()),
            remote_identity_public: Some(self.remote_identity.serialize()),
            root_key: Some(self.root_key.bytes().to_vec()),
            previous_counter: Some(self.previous_counter),
            sender_chain: self.sender_chain.as_ref().map(sender_chain_structure),
            receiver_chains: self
                .receiver_chains
                .iter()
                .map(receiver_chain_structure)
                .collect(),
            pending_pre_key: self.pending_pre_key.as_ref().map(|p| PendingPreKey {
                pre_key_id: p.pre_key_id,
                signed_pre_key_id: i32::try_from(p.signed_pre_key_id).ok(),
                base_key: Some(p.base_key.serialize()),
            }),
            remote_registration_id: Some(self.remote_registration_id),
            local_registration_id: Some(self.local_registration_id),
            needs_refresh: None,
            alice_base_key: self.alice_base_key.as_ref().map(PublicKey::serialize),
            pending_kyber_pre_key: self.pending_kyber.as_ref().map(|k| PendingKyberPreKey {
                pre_key_id: Some(k.kyber_pre_key_id),
                ciphertext: Some(k.ciphertext.clone()),
            }),
        }
    }

    /// Parses the protobuf storage form, rejecting any missing or malformed field.
    pub(crate) fn from_structure(s: &SessionStructure) -> Result<Self> {
        let version = proto::required(s.session_version)?;
        if version != 3 && version != 4 {
            return Err(Error::InvalidSessionStructure(
                "unsupported session version",
            ));
        }
        let mut state = SessionState::new(
            version,
            IdentityKey::deserialize(&proto::required(s.local_identity_public.clone())?)?,
            IdentityKey::deserialize(&proto::required(s.remote_identity_public.clone())?)?,
            RootKey::new(key32(proto::required(s.root_key.clone())?)?),
            proto::required(s.local_registration_id)?,
            proto::required(s.remote_registration_id)?,
        );
        state.previous_counter = s.previous_counter.unwrap_or(0);
        state.sender_chain = s
            .sender_chain
            .as_ref()
            .map(parse_sender_chain)
            .transpose()?;
        state.receiver_chains = s
            .receiver_chains
            .iter()
            .map(parse_receiver_chain)
            .collect::<Result<_>>()?;
        state.pending_pre_key = s
            .pending_pre_key
            .as_ref()
            .map(parse_pending_pre_key)
            .transpose()?;
        state.alice_base_key = s
            .alice_base_key
            .as_ref()
            .map(|k| PublicKey::deserialize(k))
            .transpose()?;
        state.pending_kyber = s
            .pending_kyber_pre_key
            .as_ref()
            .map(|k| {
                Ok::<_, Error>(PendingKyberMessage {
                    kyber_pre_key_id: proto::required(k.pre_key_id)?,
                    ciphertext: proto::required(k.ciphertext.clone())?,
                })
            })
            .transpose()?;
        Ok(state)
    }
}

impl fmt::Debug for SessionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionState")
            .field("version", &self.version)
            .field("receiver_chains", &self.receiver_chains.len())
            .field("has_sender_chain", &self.sender_chain.is_some())
            .finish_non_exhaustive()
    }
}

fn sender_chain_structure(chain: &SenderChain) -> Chain {
    Chain {
        sender_ratchet_key: Some(chain.ratchet_key.public_key.serialize()),
        sender_ratchet_key_private: Some(chain.ratchet_key.private_key.serialize().to_vec()),
        chain_key: Some(chain_key_structure(&chain.chain_key)),
        message_keys: Vec::new(),
    }
}

fn receiver_chain_structure(chain: &ReceiverChain) -> Chain {
    Chain {
        sender_ratchet_key: Some(chain.sender_ratchet_key.serialize()),
        sender_ratchet_key_private: None,
        chain_key: Some(chain_key_structure(&chain.chain_key)),
        message_keys: chain
            .message_keys
            .iter()
            .map(|k| MessageKey {
                index: Some(k.index()),
                cipher_key: Some(k.cipher_key().to_vec()),
                mac_key: Some(k.mac_key().to_vec()),
                iv: Some(k.iv().to_vec()),
            })
            .collect(),
    }
}

fn chain_key_structure(key: &ChainKey) -> proto::ChainKey {
    proto::ChainKey {
        index: Some(key.index()),
        key: Some(key.bytes().to_vec()),
    }
}

fn parse_chain_key(chain: &Chain) -> Result<ChainKey> {
    let structure = proto::required(chain.chain_key.clone())?;
    Ok(ChainKey::new(
        key32(proto::required(structure.key)?)?,
        proto::required(structure.index)?,
    ))
}

fn parse_sender_chain(chain: &Chain) -> Result<SenderChain> {
    let public = proto::required(chain.sender_ratchet_key.clone())?;
    let private = Zeroizing::new(proto::required(chain.sender_ratchet_key_private.clone())?);
    Ok(SenderChain {
        ratchet_key: KeyPair::from_public_and_private(&public, &private)?,
        chain_key: parse_chain_key(chain)?,
    })
}

fn parse_receiver_chain(chain: &Chain) -> Result<ReceiverChain> {
    let message_keys = chain
        .message_keys
        .iter()
        .map(|k| {
            Ok(MessageKeys::new(
                key32(proto::required(k.cipher_key.clone())?)?,
                key32(proto::required(k.mac_key.clone())?)?,
                key16(proto::required(k.iv.clone())?)?,
                proto::required(k.index)?,
            ))
        })
        .collect::<Result<VecDeque<_>>>()?;
    Ok(ReceiverChain {
        sender_ratchet_key: PublicKey::deserialize(&proto::required(
            chain.sender_ratchet_key.clone(),
        )?)?,
        chain_key: parse_chain_key(chain)?,
        message_keys,
    })
}

fn parse_pending_pre_key(p: &PendingPreKey) -> Result<PendingPreKeyMessage> {
    Ok(PendingPreKeyMessage {
        pre_key_id: p.pre_key_id,
        signed_pre_key_id: u32::try_from(proto::required(p.signed_pre_key_id)?)
            .map_err(|_| Error::InvalidSessionStructure("signed prekey id"))?,
        base_key: PublicKey::deserialize(&proto::required(p.base_key.clone())?)?,
    })
}

/// Converts a stored byte string to a 32-byte key.
pub(crate) fn key32(bytes: Vec<u8>) -> Result<[u8; 32]> {
    let bytes = Zeroizing::new(bytes);
    <[u8; 32]>::try_from(&bytes[..]).map_err(|_| Error::InvalidSessionStructure("key length"))
}

/// Converts a stored byte string to a 16-byte IV.
pub(crate) fn key16(bytes: Vec<u8>) -> Result<[u8; 16]> {
    <[u8; 16]>::try_from(&bytes[..]).map_err(|_| Error::InvalidSessionStructure("iv length"))
}

/// A record for one peer: the current state and the archived states, newest first.
#[derive(Clone, Default)]
pub struct SessionRecord {
    current: Option<SessionState>,
    previous: VecDeque<SessionState>,
}

impl SessionRecord {
    /// A record with no session yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the record has never held a session.
    pub fn is_fresh(&self) -> bool {
        self.current.is_none()
    }

    /// The current state, if any.
    pub fn state(&self) -> Option<&SessionState> {
        self.current.as_ref()
    }

    /// The current state, mutably, if any.
    pub fn state_mut(&mut self) -> Option<&mut SessionState> {
        self.current.as_mut()
    }

    /// The archived states, newest first.
    pub fn previous_states(&self) -> impl Iterator<Item = &SessionState> {
        self.previous.iter()
    }

    /// Replaces the current state, leaving the archive unchanged.
    pub fn set_state(&mut self, state: SessionState) {
        self.current = Some(state);
    }

    /// Moves the current state into the archive and leaves the record without a current state.
    pub fn archive_current_state(&mut self) {
        if let Some(state) = self.current.take() {
            self.previous.push_front(state);
            self.trim_archive();
        }
    }

    /// Replaces the archived state at `index` with `state` and makes it current, archiving the
    /// previous current state in its place. Returns `false` if there is no archived state there.
    pub(crate) fn promote_previous(&mut self, index: usize, state: SessionState) -> bool {
        if self.previous.remove(index).is_none() {
            return false;
        }
        if let Some(current) = self.current.replace(state) {
            self.previous.push_front(current);
        }
        self.trim_archive();
        true
    }

    /// Whether a current or archived state has the given version and initiator base key.
    pub fn has_session_state(&self, version: u32, base_key: &PublicKey) -> bool {
        self.current
            .iter()
            .chain(self.previous.iter())
            .any(|s| s.version == version && s.has_alice_base_key(base_key))
    }

    fn trim_archive(&mut self) {
        while self.previous.len() > MAX_ARCHIVED_STATES {
            self.previous.pop_back();
        }
    }

    /// Serializes as `RecordStructure`. The result holds private keys.
    pub fn serialize(&self) -> Zeroizing<Vec<u8>> {
        let structure = RecordStructure {
            current_session: self.current.as_ref().map(SessionState::to_structure),
            previous_sessions: self
                .previous
                .iter()
                .map(SessionState::to_structure)
                .collect(),
        };
        Zeroizing::new(structure.encode_to_vec())
    }

    /// Parses the output of [`SessionRecord::serialize`].
    pub fn deserialize(bytes: &[u8]) -> Result<Self> {
        let structure: RecordStructure = proto::decode(bytes)?;
        let current = structure
            .current_session
            .as_ref()
            .map(SessionState::from_structure)
            .transpose()?;
        let previous = structure
            .previous_sessions
            .iter()
            .map(SessionState::from_structure)
            .collect::<Result<VecDeque<_>>>()?;
        if previous.len() > MAX_ARCHIVED_STATES {
            return Err(Error::InvalidSessionStructure("too many archived states"));
        }
        Ok(Self { current, previous })
    }
}

impl fmt::Debug for SessionRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionRecord")
            .field("current", &self.current)
            .field("archived", &self.previous.len())
            .finish()
    }
}
