//! Sender keys for group messages: one sending chain per (group, sender), shared once with each
//! member through a distribution message.
//!
//! Port of libsignal `groups/{GroupCipher,GroupSessionBuilder}.go`, `groups/ratchet/*.go` and
//! `groups/state/record/*.go` (AGPL-3.0-only, Signal Messenger LLC).
//!
//! Message keys: `seed = HMAC(chain, 0x01)`, then `HKDF(seed, salt = None, "WhisperGroup", 48)`
//! gives the 16-byte IV and the 32-byte cipher key. The next chain key is `HMAC(chain, 0x02)`.
//! Group decryption works on a clone of the state, so a failed signature or MAC check leaves the
//! store unchanged.

use std::collections::VecDeque;
use std::fmt;

use prost::Message;
use rand::{CryptoRng, RngCore};
use zeroize::Zeroizing;

use crate::ProtocolAddress;
use crate::cipher::{aes_cbc_decrypt, aes_cbc_encrypt};
use crate::curve::{KeyPair, PrivateKey, PublicKey};
use crate::error::{Error, Result};
use crate::kdf::{hkdf_sha256, hmac_sha256};
use crate::message::{SenderKeyDistributionMessage, SenderKeyMessage};
use crate::proto::{self, SenderKeyRecordStructure, SenderKeyStateStructure};
use crate::protocol::CiphertextMessageType;
use crate::session::{MAX_FORWARD_JUMPS, key32};
use crate::store::SenderKeyStore;

/// Maximum number of sender key states kept per (group, sender).
pub const MAX_SENDER_KEY_STATES: usize = 5;
/// Maximum number of skipped message keys kept per state.
pub const MAX_SENDER_MESSAGE_KEYS: usize = 2000;
/// HKDF info for group message keys.
const GROUP_KEYS_INFO: &[u8] = b"WhisperGroup";
/// Seed that turns a sender chain key into its message seed.
const MESSAGE_KEY_SEED: [u8; 1] = [0x01];
/// Seed that turns a sender chain key into the next one.
const CHAIN_KEY_SEED: [u8; 1] = [0x02];
/// Length of the derived group key material: 16-byte IV plus 32-byte cipher key.
const GROUP_KEY_MATERIAL_LENGTH: usize = 48;

/// Identifies a sender key record: a group and the member who owns the sending chain.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SenderKeyName {
    group_id: String,
    sender: ProtocolAddress,
}

impl SenderKeyName {
    /// Names the sender key of `sender` in `group_id`.
    pub fn new(group_id: impl Into<String>, sender: ProtocolAddress) -> Self {
        Self {
            group_id: group_id.into(),
            sender,
        }
    }

    /// The group identifier.
    pub fn group_id(&self) -> &str {
        &self.group_id
    }

    /// The sender of the chain.
    pub fn sender(&self) -> &ProtocolAddress {
        &self.sender
    }
}

impl fmt::Debug for SenderKeyName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SenderKeyName [redacted]")
    }
}

/// The key, IV and seed for one group message.
#[derive(Clone)]
pub struct SenderMessageKey {
    iteration: u32,
    seed: Zeroizing<[u8; 32]>,
    iv: [u8; 16],
    cipher_key: Zeroizing<[u8; 32]>,
}

impl SenderMessageKey {
    /// Derives the IV and cipher key from a message seed.
    pub fn from_seed(iteration: u32, seed: [u8; 32]) -> Result<Self> {
        let material = hkdf_sha256(&seed, None, GROUP_KEYS_INFO, GROUP_KEY_MATERIAL_LENGTH)?;
        let iv: [u8; 16] = material
            .get(..16)
            .and_then(|s| s.try_into().ok())
            .ok_or(Error::InvalidArgument("group key material"))?;
        let cipher_key: [u8; 32] = material
            .get(16..48)
            .and_then(|s| s.try_into().ok())
            .ok_or(Error::InvalidArgument("group key material"))?;
        Ok(Self {
            iteration,
            seed: Zeroizing::new(seed),
            iv,
            cipher_key: Zeroizing::new(cipher_key),
        })
    }

    /// The iteration (counter) of this message key.
    pub fn iteration(&self) -> u32 {
        self.iteration
    }

    /// The CBC initialisation vector.
    pub fn iv(&self) -> &[u8; 16] {
        &self.iv
    }

    /// The AES-256 cipher key.
    pub fn cipher_key(&self) -> &[u8; 32] {
        &self.cipher_key
    }
}

impl fmt::Debug for SenderMessageKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SenderMessageKey")
            .field("iteration", &self.iteration)
            .finish_non_exhaustive()
    }
}

/// A sender chain key: the seed and the iteration it belongs to.
#[derive(Clone)]
pub struct SenderChainKey {
    iteration: u32,
    seed: Zeroizing<[u8; 32]>,
}

impl SenderChainKey {
    /// Wraps a chain seed at an iteration.
    pub fn new(iteration: u32, seed: [u8; 32]) -> Self {
        Self {
            iteration,
            seed: Zeroizing::new(seed),
        }
    }

    /// The iteration of this chain key.
    pub fn iteration(&self) -> u32 {
        self.iteration
    }

    /// The raw seed. Callers must treat the result as secret.
    pub fn seed(&self) -> &[u8; 32] {
        &self.seed
    }

    /// The message key for this iteration.
    pub fn message_key(&self) -> Result<SenderMessageKey> {
        let seed = hmac_sha256(&self.seed[..], &[&MESSAGE_KEY_SEED])?;
        SenderMessageKey::from_seed(self.iteration, *seed)
    }

    /// The chain key for the next iteration.
    pub fn next(&self) -> Result<SenderChainKey> {
        let next = hmac_sha256(&self.seed[..], &[&CHAIN_KEY_SEED])?;
        let iteration = self
            .iteration
            .checked_add(1)
            .ok_or(Error::InvalidArgument("sender chain iteration overflow"))?;
        Ok(SenderChainKey::new(iteration, *next))
    }
}

impl fmt::Debug for SenderChainKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SenderChainKey")
            .field("iteration", &self.iteration)
            .finish_non_exhaustive()
    }
}

/// One sending chain of a sender key record.
#[derive(Clone)]
pub struct SenderKeyState {
    key_id: u32,
    chain_key: SenderChainKey,
    signing_public: PublicKey,
    signing_private: Option<PrivateKey>,
    message_keys: VecDeque<SenderMessageKey>,
}

impl SenderKeyState {
    /// A state for our own sending chain, which can encrypt.
    pub fn new_own(key_id: u32, chain_key: SenderChainKey, signing: KeyPair) -> Self {
        Self {
            key_id,
            chain_key,
            signing_public: signing.public_key,
            signing_private: Some(signing.private_key),
            message_keys: VecDeque::new(),
        }
    }

    /// A state for a member's sending chain, learned from a distribution message. It can decrypt.
    pub fn new_remote(key_id: u32, chain_key: SenderChainKey, signing_public: PublicKey) -> Self {
        Self {
            key_id,
            chain_key,
            signing_public,
            signing_private: None,
            message_keys: VecDeque::new(),
        }
    }

    /// The sender key id.
    pub fn key_id(&self) -> u32 {
        self.key_id
    }

    /// Removes and returns the saved key for `iteration`, if there is one.
    fn take_message_key(&mut self, iteration: u32) -> Option<SenderMessageKey> {
        let position = self
            .message_keys
            .iter()
            .position(|k| k.iteration == iteration)?;
        self.message_keys.remove(position)
    }

    /// Saves the key of a skipped message, dropping the oldest beyond the limit.
    fn save_message_key(&mut self, key: SenderMessageKey) {
        self.message_keys.push_back(key);
        while self.message_keys.len() > MAX_SENDER_MESSAGE_KEYS {
            self.message_keys.pop_front();
        }
    }

    /// Converts to the protobuf storage form.
    fn to_structure(&self) -> SenderKeyStateStructure {
        SenderKeyStateStructure {
            sender_key_id: Some(self.key_id),
            sender_chain_key: Some(proto::SenderChainKey {
                iteration: Some(self.chain_key.iteration),
                seed: Some(self.chain_key.seed.to_vec()),
            }),
            sender_signing_key: Some(proto::SenderSigningKey {
                public: Some(self.signing_public.serialize()),
                private: self
                    .signing_private
                    .as_ref()
                    .map(|k| k.serialize().to_vec()),
            }),
            sender_message_keys: self
                .message_keys
                .iter()
                .map(|k| proto::SenderMessageKey {
                    iteration: Some(k.iteration),
                    seed: Some(k.seed.to_vec()),
                })
                .collect(),
        }
    }

    /// Parses the protobuf storage form.
    fn from_structure(s: &SenderKeyStateStructure) -> Result<Self> {
        let chain = proto::required(s.sender_chain_key.clone())?;
        let signing = proto::required(s.sender_signing_key.clone())?;
        let signing_public = PublicKey::deserialize(&proto::required(signing.public)?)?;
        let signing_private = signing
            .private
            .map(|bytes| PrivateKey::deserialize(&Zeroizing::new(bytes)))
            .transpose()?;
        let message_keys = s
            .sender_message_keys
            .iter()
            .map(|k| {
                let seed = key32(proto::required(k.seed.clone())?)?;
                SenderMessageKey::from_seed(proto::required(k.iteration)?, seed)
            })
            .collect::<Result<VecDeque<_>>>()?;
        Ok(Self {
            key_id: proto::required(s.sender_key_id)?,
            chain_key: SenderChainKey::new(
                proto::required(chain.iteration)?,
                key32(proto::required(chain.seed)?)?,
            ),
            signing_public,
            signing_private,
            message_keys,
        })
    }
}

impl fmt::Debug for SenderKeyState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SenderKeyState")
            .field("key_id", &self.key_id)
            .field("iteration", &self.chain_key.iteration)
            .finish_non_exhaustive()
    }
}

/// All sender key states for one (group, sender), newest first.
#[derive(Clone, Default, Debug)]
pub struct SenderKeyRecord {
    states: Vec<SenderKeyState>,
}

impl SenderKeyRecord {
    /// An empty record.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the record has no state.
    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    /// The newest state, which is the one used for sending.
    pub fn current_state(&self) -> Option<&SenderKeyState> {
        self.states.first()
    }

    /// The state with `key_id`, if any.
    pub fn state_for_id(&self, key_id: u32) -> Option<&SenderKeyState> {
        self.states.iter().find(|s| s.key_id == key_id)
    }

    /// Adds a state as the newest. A state with the same key id is replaced, and the oldest
    /// state is dropped beyond [`MAX_SENDER_KEY_STATES`].
    pub fn add_state(&mut self, state: SenderKeyState) {
        self.states.retain(|s| s.key_id != state.key_id);
        self.states.insert(0, state);
        self.states.truncate(MAX_SENDER_KEY_STATES);
    }

    /// Replaces the state that has the same key id as `state`.
    fn replace_state(&mut self, state: SenderKeyState) -> Result<()> {
        let slot = self
            .states
            .iter_mut()
            .find(|s| s.key_id == state.key_id)
            .ok_or(Error::NoSenderKeyState)?;
        *slot = state;
        Ok(())
    }

    /// Serializes as `SenderKeyRecordStructure`. The result holds private signing keys.
    pub fn serialize(&self) -> Zeroizing<Vec<u8>> {
        let structure = SenderKeyRecordStructure {
            sender_key_states: self
                .states
                .iter()
                .map(SenderKeyState::to_structure)
                .collect(),
        };
        Zeroizing::new(structure.encode_to_vec())
    }

    /// Parses the output of [`SenderKeyRecord::serialize`].
    pub fn deserialize(bytes: &[u8]) -> Result<Self> {
        let structure: SenderKeyRecordStructure = proto::decode(bytes)?;
        if structure.sender_key_states.len() > MAX_SENDER_KEY_STATES {
            return Err(Error::InvalidSenderKeySession);
        }
        let states = structure
            .sender_key_states
            .iter()
            .map(SenderKeyState::from_structure)
            .collect::<Result<_>>()?;
        Ok(Self { states })
    }
}

/// Creates our sending chain for `name` if needed, and returns the distribution message that
/// shares it with the group.
pub fn create_sender_key_distribution_message<S, R>(
    store: &mut S,
    name: &SenderKeyName,
    csprng: &mut R,
) -> Result<SenderKeyDistributionMessage>
where
    S: SenderKeyStore + ?Sized,
    R: CryptoRng + RngCore,
{
    let mut record = store.load_sender_key(name)?.unwrap_or_default();
    if record.is_empty() {
        let key_id = csprng.next_u32() & 0x7FFF_FFFF;
        let mut seed = Zeroizing::new([0u8; 32]);
        csprng.fill_bytes(&mut *seed);
        let signing = KeyPair::generate(csprng);
        record.add_state(SenderKeyState::new_own(
            key_id,
            SenderChainKey::new(0, *seed),
            signing,
        ));
        store.store_sender_key(name, &record)?;
    }
    let state = record.current_state().ok_or(Error::NoSenderKeyState)?;
    SenderKeyDistributionMessage::new(
        state.key_id,
        state.chain_key.iteration,
        *state.chain_key.seed,
        state.signing_public,
    )
}

/// Records a member's sending chain from their distribution message.
pub fn process_sender_key_distribution_message<S>(
    store: &mut S,
    name: &SenderKeyName,
    message: &SenderKeyDistributionMessage,
) -> Result<()>
where
    S: SenderKeyStore + ?Sized,
{
    let mut record = store.load_sender_key(name)?.unwrap_or_default();
    record.add_state(SenderKeyState::new_remote(
        message.key_id(),
        SenderChainKey::new(message.iteration(), *message.chain_key()),
        *message.signing_key(),
    ));
    store.store_sender_key(name, &record)?;
    Ok(())
}

/// Encrypts `plaintext` with our sending chain for `name` and advances it.
pub fn group_encrypt<S, R>(
    store: &mut S,
    name: &SenderKeyName,
    plaintext: &[u8],
    csprng: &mut R,
) -> Result<SenderKeyMessage>
where
    S: SenderKeyStore + ?Sized,
    R: CryptoRng + RngCore,
{
    let mut record = store
        .load_sender_key(name)?
        .ok_or(Error::NoSenderKeyState)?;
    let state = record.states.first_mut().ok_or(Error::NoSenderKeyState)?;
    let signing = state
        .signing_private
        .clone()
        .ok_or(Error::InvalidSenderKeySession)?;
    let key = state.chain_key.message_key()?;
    let ciphertext = aes_cbc_encrypt(&key.cipher_key, &key.iv, plaintext)?;
    let message = SenderKeyMessage::new(state.key_id, key.iteration, ciphertext, &signing, csprng)?;
    state.chain_key = state.chain_key.next()?;
    store.store_sender_key(name, &record)?;
    Ok(message)
}

/// Decrypts a group message from `name`'s sending chain.
///
/// Verifies the signature first. Then advances the chain, saving the keys of skipped messages
/// (up to [`MAX_SENDER_MESSAGE_KEYS`]). A repeated iteration fails with
/// [`Error::DuplicatedMessage`].
pub fn group_decrypt<S>(
    store: &mut S,
    name: &SenderKeyName,
    message: &SenderKeyMessage,
) -> Result<Vec<u8>>
where
    S: SenderKeyStore + ?Sized,
{
    let mut record = store
        .load_sender_key(name)?
        .ok_or(Error::NoSenderKeyState)?;
    let state = record
        .state_for_id(message.key_id())
        .ok_or(Error::NoSenderKeyState)?
        .clone();
    message.verify_signature(&state.signing_public)?;
    let (key, updated) = sender_message_key_for(state, message.iteration())?;
    let plaintext = aes_cbc_decrypt(&key.cipher_key, &key.iv, message.ciphertext())?;
    record.replace_state(updated)?;
    store.store_sender_key(name, &record)?;
    Ok(plaintext)
}

/// Returns the key for `iteration`, advancing a copy of the state to get it.
fn sender_message_key_for(
    mut state: SenderKeyState,
    iteration: u32,
) -> Result<(SenderMessageKey, SenderKeyState)> {
    let current = state.chain_key.iteration;
    if current > iteration {
        return match state.take_message_key(iteration) {
            Some(key) => Ok((key, state)),
            None => Err(Error::DuplicatedMessage(current, iteration)),
        };
    }
    if iteration - current > MAX_FORWARD_JUMPS {
        return Err(Error::InvalidMessage(
            CiphertextMessageType::SenderKey,
            "iteration is too far ahead of the chain",
        ));
    }
    while state.chain_key.iteration < iteration {
        let skipped = state.chain_key.message_key()?;
        state.save_message_key(skipped);
        state.chain_key = state.chain_key.next()?;
    }
    let key = state.chain_key.message_key()?;
    state.chain_key = state.chain_key.next()?;
    Ok((key, state))
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    use super::*;

    #[test]
    fn message_key_derivation_matches_manual_steps() {
        // seed = HMAC(0x05*32, 0x01); material = HKDF(seed, None, "WhisperGroup", 48).
        let chain = SenderChainKey::new(3, [5u8; 32]);
        let key = chain.message_key().unwrap();
        let seed = hmac_sha256(&[5u8; 32], &[&[1]]).unwrap();
        let material = hkdf_sha256(&seed[..], None, b"WhisperGroup", 48).unwrap();
        assert_eq!(key.iv()[..], material[..16]);
        assert_eq!(key.cipher_key()[..], material[16..48]);
        assert_eq!(key.iteration(), 3);
        let next = chain.next().unwrap();
        assert_eq!(next.iteration(), 4);
        assert_eq!(
            next.seed()[..],
            hmac_sha256(&[5u8; 32], &[&[2]]).unwrap()[..]
        );
    }

    #[test]
    fn known_answer_group_key() {
        // Computed independently with Python's hmac/hashlib, following libsignal groups/ratchet:
        // seed = HMAC(0x05*32, 0x01); material = HKDF(seed, salt=zeros, "WhisperGroup", 48).
        let key = SenderChainKey::new(0, [5u8; 32]).message_key().unwrap();
        assert_eq!(hex::encode(key.iv()), "7cc83fb108387b9f09360ffc75dfc5b7");
        assert_eq!(
            hex::encode(key.cipher_key()),
            "70f9c01051d780cdbace2f8003af041c0a5c28994e630cf9e184554b8dd1c4b1"
        );
    }

    #[test]
    fn record_limits_and_round_trip() {
        let mut rng = StdRng::seed_from_u64(30);
        let mut record = SenderKeyRecord::new();
        for id in 0..7u32 {
            let signing = KeyPair::generate(&mut rng);
            record.add_state(SenderKeyState::new_own(
                id,
                SenderChainKey::new(0, [id as u8; 32]),
                signing,
            ));
        }
        assert_eq!(record.states.len(), MAX_SENDER_KEY_STATES);
        assert_eq!(record.current_state().unwrap().key_id(), 6);
        assert!(record.state_for_id(0).is_none());
        let restored = SenderKeyRecord::deserialize(&record.serialize()).unwrap();
        assert_eq!(restored.states.len(), 5);
        assert_eq!(restored.current_state().unwrap().key_id(), 6);
        assert!(restored.current_state().unwrap().signing_private.is_some());
        assert!(SenderKeyRecord::deserialize(&[0xff, 0xff]).is_err());
    }
}
