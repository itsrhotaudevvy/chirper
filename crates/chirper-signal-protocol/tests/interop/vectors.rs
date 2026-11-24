//! Loads the Go-produced interop vectors and rebuilds Bob's stores from their private keys.
//!
//! Shared by `tests/interop.rs` and `examples/interop_reply.rs` (included with `#[path]`).
//! The vectors are written by `tests/interop/generate` (Go libsignal) and checked in.

#![allow(dead_code)]

use chirper_signal_protocol::ProtocolAddress;
use chirper_signal_protocol::curve::{KeyPair, PrivateKey, PublicKey};
use chirper_signal_protocol::identity::{IdentityKey, IdentityKeyPair};
use chirper_signal_protocol::memory::InMemoryProtocolStore;
use chirper_signal_protocol::prekey::{PreKeyRecord, SignedPreKeyRecord};
use chirper_signal_protocol::session::SessionRecord;
use chirper_signal_protocol::store::{PreKeyStore, SessionStore, SignedPreKeyStore};
use serde::{Deserialize, Serialize};

/// Error type for the vector helpers: they only run in tests and examples.
pub type BoxError = Box<dyn std::error::Error>;

/// The checked-in vectors file.
pub const VECTORS_JSON: &str = include_str!("go_vectors.json");

/// The whole vectors file as written by the Go generator.
#[derive(Debug, Deserialize)]
pub struct Vectors {
    pub library: String,
    pub alice: Party,
    pub bob: Bob,
    pub messages: Vec<Message>,
    pub group: Group,
}

/// A party's public identity and its private identity key.
#[derive(Debug, Deserialize)]
pub struct Party {
    pub name: String,
    pub device_id: u32,
    pub registration_id: u32,
    pub identity_public: String,
    pub identity_private: String,
}

/// Bob's identity plus the one-time and signed prekeys, all with their private halves.
#[derive(Debug, Deserialize)]
pub struct Bob {
    #[serde(flatten)]
    pub party: Party,
    pub pre_key_id: u32,
    pub pre_key_public: String,
    pub pre_key_private: String,
    pub signed_pre_key_id: u32,
    pub signed_pre_key_timestamp: u64,
    pub signed_pre_key_public: String,
    pub signed_pre_key_private: String,
    pub signed_pre_key_signature: String,
}

/// One one-to-one message from Go. `receiver_session` is the receiver's libsignal session record
/// just before the receiver decrypted this message (absent for the prekey message).
#[derive(Debug, Deserialize)]
pub struct Message {
    pub index: usize,
    pub from: String,
    pub to: String,
    pub kind: String,
    pub plaintext: String,
    pub ciphertext: String,
    pub receiver_session: Option<String>,
}

/// The group sender key distribution and the messages sent under it.
#[derive(Debug, Deserialize)]
pub struct Group {
    pub group_id: String,
    pub sender: String,
    pub distribution_message: String,
    pub messages: Vec<GroupMessage>,
}

/// One group message from Go.
#[derive(Debug, Deserialize)]
pub struct GroupMessage {
    pub plaintext: String,
    pub ciphertext: String,
}

/// The replies Rust produced, for the Go `verify` mode.
#[derive(Debug, Serialize)]
pub struct Replies {
    pub replies: Vec<Reply>,
}

/// One Rust-encrypted reply. Go decrypts it starting from `messages[decrypt_with].receiver_session`.
#[derive(Debug, Serialize)]
pub struct Reply {
    pub decrypt_with: usize,
    pub plaintext: String,
    pub ciphertext: String,
}

/// Parses the checked-in vectors.
pub fn load() -> Result<Vectors, BoxError> {
    Ok(serde_json::from_str(VECTORS_JSON)?)
}

/// Decodes a hex field.
pub fn bytes(hex_text: &str) -> Result<Vec<u8>, BoxError> {
    Ok(hex::decode(hex_text)?)
}

/// Alice's protocol address, as Bob knows her.
pub fn alice_address(vectors: &Vectors) -> ProtocolAddress {
    ProtocolAddress::new(vectors.alice.name.clone(), 1)
}

/// Bob's protocol address, as Alice knows him.
pub fn bob_address(vectors: &Vectors) -> ProtocolAddress {
    ProtocolAddress::new(vectors.bob.party.name.clone(), 1)
}

/// Builds a public/private key pair from hex and checks that the private key derives the public.
fn key_pair(public_hex: &str, private_hex: &str) -> Result<KeyPair, BoxError> {
    let public = PublicKey::from_djb_public_key_bytes(&bytes(public_hex)?)?;
    let private = PrivateKey::deserialize(&bytes(private_hex)?)?;
    if private.public_key() != public {
        return Err("vector private key does not derive its public key".into());
    }
    Ok(KeyPair::new(public, private))
}

/// Alice's identity pair, rebuilt from her private key.
pub fn identity_pair(party: &Party) -> Result<IdentityKeyPair, BoxError> {
    let pair = key_pair(&party.identity_public, &party.identity_private)?;
    let (public, private) = (pair.public_key, pair.private_key);
    Ok(IdentityKeyPair::new(IdentityKey::new(public), private))
}

/// A store for `party` with no sessions and no prekeys.
pub fn empty_store(party: &Party) -> Result<InMemoryProtocolStore, BoxError> {
    Ok(InMemoryProtocolStore::new(
        identity_pair(party)?,
        party.registration_id,
    ))
}

/// A store for Bob that holds exactly the private keys in the vectors: his identity, his
/// one-time prekey and his signed prekey. This is what the prekey message decrypts against.
pub fn bob_store(vectors: &Vectors) -> Result<InMemoryProtocolStore, BoxError> {
    let bob = &vectors.bob;
    let mut store = empty_store(&bob.party)?;
    let pre_key = key_pair(&bob.pre_key_public, &bob.pre_key_private)?;
    store.store_pre_key(&PreKeyRecord::new(bob.pre_key_id, pre_key))?;
    let signed = key_pair(&bob.signed_pre_key_public, &bob.signed_pre_key_private)?;
    let signature: [u8; 64] = bytes(&bob.signed_pre_key_signature)?
        .try_into()
        .map_err(|_| "signed prekey signature is not 64 bytes")?;
    store.store_signed_pre_key(&SignedPreKeyRecord::new(
        bob.signed_pre_key_id,
        bob.signed_pre_key_timestamp,
        signed,
        signature,
    ))?;
    Ok(store)
}

/// Loads the libsignal session record recorded for `message` into `store` under `address`.
pub fn load_receiver_session(
    store: &mut InMemoryProtocolStore,
    address: &ProtocolAddress,
    message: &Message,
) -> Result<(), BoxError> {
    let snapshot = message
        .receiver_session
        .as_deref()
        .ok_or("message has no receiver session snapshot")?;
    let record = SessionRecord::deserialize(&bytes(snapshot)?)?;
    store.store_session(address, &record)?;
    Ok(())
}

/// The message with the given index, or an error if the vectors lack it.
pub fn message(vectors: &Vectors, index: usize) -> Result<&Message, BoxError> {
    vectors
        .messages
        .get(index)
        .ok_or_else(|| format!("vectors have no message {index}").into())
}
