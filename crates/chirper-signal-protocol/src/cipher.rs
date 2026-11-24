//! One-to-one encryption and decryption over a stored session.
//!
//! Port of libsignal `session/SessionCipher.go` and `cipher/Cbc.go` (AGPL-3.0-only, Signal
//! Messenger LLC). Payloads are AES-256-CBC with PKCS#7 padding, and the IV and cipher key come
//! from the message key of the chain.
//!
//! Decryption is transactional. Every attempt runs on a clone of the session record. The record
//! is committed to the store only after the MAC and the padding check succeed, together with
//! the identity and any consumed one-time or Kyber prekey. A failed attempt therefore leaves
//! the store untouched.

use cbc::cipher::block_padding::Pkcs7;
use cbc::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit};
use rand::{CryptoRng, RngCore};

use crate::ProtocolAddress;
use crate::curve::KeyPair;
use crate::error::{Error, Result};
use crate::kdf::{ChainKey, MessageKeys};
use crate::message::{CiphertextMessage, PreKeySignalMessage, PreKeySignalParts, SignalMessage};
use crate::protocol::CiphertextMessageType;
use crate::session::{MAX_FORWARD_JUMPS, SessionRecord, SessionState};
use crate::session_builder::{self, PreKeyOutcome};
use crate::store::ProtocolStore;

/// Block size of AES.
const AES_BLOCK_LENGTH: usize = 16;

/// AES-256-CBC encryption with PKCS#7 padding.
pub fn aes_cbc_encrypt(key: &[u8; 32], iv: &[u8; 16], plaintext: &[u8]) -> Result<Vec<u8>> {
    let encryptor = cbc::Encryptor::<aes::Aes256>::new_from_slices(key, iv)
        .map_err(|_| Error::InvalidArgument("aes-cbc key or iv"))?;
    let len = plaintext.len();
    // PKCS#7 always adds between 1 and 16 bytes, so the buffer holds the next full block.
    let mut buf = vec![0u8; (len / AES_BLOCK_LENGTH + 1) * AES_BLOCK_LENGTH];
    buf[..len].copy_from_slice(plaintext);
    let ciphertext = encryptor
        .encrypt_padded::<Pkcs7>(&mut buf, len)
        .map_err(|_| Error::InvalidArgument("aes-cbc buffer"))?;
    Ok(ciphertext.to_vec())
}

/// AES-256-CBC decryption, rejecting malformed PKCS#7 padding.
pub fn aes_cbc_decrypt(key: &[u8; 32], iv: &[u8; 16], ciphertext: &[u8]) -> Result<Vec<u8>> {
    let decryptor = cbc::Decryptor::<aes::Aes256>::new_from_slices(key, iv)
        .map_err(|_| Error::InvalidArgument("aes-cbc key or iv"))?;
    let mut buf = ciphertext.to_vec();
    let len = decryptor
        .decrypt_padded::<Pkcs7>(&mut buf)
        .map_err(|_| Error::InvalidMessage(CiphertextMessageType::Whisper, "bad padding"))?
        .len();
    buf.truncate(len);
    Ok(buf)
}

/// Encrypts `plaintext` for the peer at `address`, advancing the sending chain.
///
/// The result is a [`CiphertextMessage::PreKey`] until the peer replies, and a
/// [`CiphertextMessage::Signal`] afterwards.
pub fn encrypt<S>(
    store: &mut S,
    address: &ProtocolAddress,
    plaintext: &[u8],
) -> Result<CiphertextMessage>
where
    S: ProtocolStore + ?Sized,
{
    let mut record = load_record(&*store, address)?;
    let state = record
        .state_mut()
        .ok_or_else(|| Error::SessionNotFound(address.clone()))?;
    let SessionState {
        version,
        local_identity,
        remote_identity,
        previous_counter,
        sender_chain,
        pending_pre_key,
        pending_kyber,
        local_registration_id,
        ..
    } = state;
    let chain = sender_chain.as_mut().ok_or(Error::InvalidState(
        "encrypt",
        "session has no sending chain",
    ))?;
    let keys = chain.chain_key.message_keys()?;
    let ciphertext = aes_cbc_encrypt(keys.cipher_key(), keys.iv(), plaintext)?;
    let signal = SignalMessage::new(
        *version,
        keys.index(),
        *previous_counter,
        keys.mac_key(),
        chain.ratchet_key.public_key,
        ciphertext,
        local_identity,
        remote_identity,
    )?;
    let message = match pending_pre_key {
        Some(pending) => CiphertextMessage::PreKey(PreKeySignalMessage::new(PreKeySignalParts {
            message_version: *version,
            registration_id: *local_registration_id,
            pre_key_id: pending.pre_key_id,
            signed_pre_key_id: pending.signed_pre_key_id,
            base_key: pending.base_key,
            identity_key: local_identity.clone(),
            message: &signal,
            kyber: pending_kyber
                .as_ref()
                .map(|k| (k.kyber_pre_key_id, k.ciphertext.clone())),
        })?),
        None => CiphertextMessage::Signal(signal),
    };
    chain.chain_key = chain.chain_key.next()?;
    let remote = remote_identity.clone();
    commit(store, address, &record, &remote)?;
    Ok(message)
}

/// Decrypts a Double Ratchet message from `address`.
///
/// Tries the current state, then each archived state. A state that decrypts is promoted.
/// Returns [`Error::DuplicatedMessage`] for a counter that was already consumed.
pub fn decrypt_signal_message<S, R>(
    store: &mut S,
    address: &ProtocolAddress,
    message: &SignalMessage,
    csprng: &mut R,
) -> Result<Vec<u8>>
where
    S: ProtocolStore + ?Sized,
    R: CryptoRng + RngCore,
{
    let mut record = load_record(&*store, address)?;
    let plaintext = decrypt_with_record(&mut record, message, csprng)?;
    let remote = record
        .state()
        .ok_or_else(|| Error::SessionNotFound(address.clone()))?
        .remote_identity()
        .clone();
    commit(store, address, &record, &remote)?;
    Ok(plaintext)
}

/// Decrypts a session-establishing message from `address`, starting a session if needed.
///
/// On success the one-time prekey and the Kyber prekey the message used are removed or marked
/// used, so that they cannot be accepted again.
pub fn decrypt_prekey_message<S, R>(
    store: &mut S,
    address: &ProtocolAddress,
    message: &PreKeySignalMessage,
    csprng: &mut R,
) -> Result<Vec<u8>>
where
    S: ProtocolStore + ?Sized,
    R: CryptoRng + RngCore,
{
    let mut record = load_record(&*store, address)?;
    let outcome = session_builder::process_prekey_message(&*store, &mut record, address, message)?;
    let plaintext = decrypt_with_record(&mut record, message.message(), csprng)?;
    commit(store, address, &record, message.identity_key())?;
    consume_prekeys(store, outcome)?;
    Ok(plaintext)
}

fn load_record<S>(store: &S, address: &ProtocolAddress) -> Result<SessionRecord>
where
    S: ProtocolStore + ?Sized,
{
    Ok(store.load_session(address)?.unwrap_or_default())
}

/// Checks the peer's identity, saves it, and stores the session record.
fn commit<S>(
    store: &mut S,
    address: &ProtocolAddress,
    record: &SessionRecord,
    remote_identity: &crate::identity::IdentityKey,
) -> Result<()>
where
    S: ProtocolStore + ?Sized,
{
    if !store.is_trusted_identity(address, remote_identity)? {
        return Err(Error::UntrustedIdentity(address.clone()));
    }
    store.save_identity(address, remote_identity)?;
    store.store_session(address, record)?;
    Ok(())
}

fn consume_prekeys<S>(store: &mut S, outcome: PreKeyOutcome) -> Result<()>
where
    S: ProtocolStore + ?Sized,
{
    if let Some(id) = outcome.pre_key_id {
        store.remove_pre_key(id)?;
    }
    if let Some(id) = outcome.kyber_pre_key_id {
        store.mark_kyber_pre_key_used(id)?;
    }
    Ok(())
}

/// Tries the current state, then the archived states, on clones of `record`.
fn decrypt_with_record<R>(
    record: &mut SessionRecord,
    message: &SignalMessage,
    csprng: &mut R,
) -> Result<Vec<u8>>
where
    R: CryptoRng + RngCore,
{
    let current = record
        .state()
        .ok_or(Error::InvalidState(
            "decrypt",
            "session has no current state",
        ))?
        .clone();
    let first_error = match decrypt_with_state(current, message, csprng) {
        Ok((plaintext, updated)) => {
            record.set_state(updated);
            return Ok(plaintext);
        }
        Err(e @ Error::DuplicatedMessage(..)) => return Err(e),
        Err(e) => e,
    };
    let archived: Vec<SessionState> = record.previous_states().cloned().collect();
    for (index, state) in archived.into_iter().enumerate() {
        match decrypt_with_state(state, message, csprng) {
            Ok((plaintext, updated)) => {
                record.promote_previous(index, updated);
                return Ok(plaintext);
            }
            Err(e @ Error::DuplicatedMessage(..)) => return Err(e),
            Err(_) => {}
        }
    }
    Err(first_error)
}

/// Decrypts with one state. Returns the plaintext and the updated state, which the caller
/// commits only on success.
fn decrypt_with_state<R>(
    mut state: SessionState,
    message: &SignalMessage,
    csprng: &mut R,
) -> Result<(Vec<u8>, SessionState)>
where
    R: CryptoRng + RngCore,
{
    if state.sender_chain.is_none() {
        return Err(Error::InvalidState(
            "decrypt",
            "session has no sending chain",
        ));
    }
    if message.message_version() != state.version {
        return Err(Error::InvalidMessage(
            CiphertextMessageType::Whisper,
            "message version does not match session",
        ));
    }
    let their_ratchet = message.sender_ratchet_key();
    let (chain_index, chain_key) = get_or_create_chain(&mut state, their_ratchet, csprng)?;
    let keys = get_or_create_message_keys(&mut state, chain_index, chain_key, message.counter())?;
    message.verify_mac(
        &state.remote_identity,
        &state.local_identity,
        keys.mac_key(),
    )?;
    let plaintext = aes_cbc_decrypt(keys.cipher_key(), keys.iv(), message.ciphertext())?;
    state.clear_pending();
    Ok((plaintext, state))
}

/// Returns the receiving chain for `their_ratchet`, performing a DH ratchet step if we have
/// none. Returns the chain's index and its current chain key.
fn get_or_create_chain<R>(
    state: &mut SessionState,
    their_ratchet: &crate::curve::PublicKey,
    csprng: &mut R,
) -> Result<(usize, ChainKey)>
where
    R: CryptoRng + RngCore,
{
    if let Some(index) = state.receiver_chain_index(their_ratchet) {
        let key = state.receiver_chains[index].chain_key.clone();
        return Ok((index, key));
    }
    let sender = state.sender_chain.clone().ok_or(Error::InvalidState(
        "decrypt",
        "session has no sending chain",
    ))?;
    let (receiver_root, receiver_chain) = state
        .root_key
        .create_chain(their_ratchet, &sender.ratchet_key)?;
    let new_ratchet = KeyPair::generate(csprng);
    let (sender_root, sender_chain_key) =
        receiver_root.create_chain(their_ratchet, &new_ratchet)?;

    state.root_key = sender_root;
    state.add_receiver_chain(*their_ratchet, receiver_chain.clone());
    state.previous_counter = sender.chain_key.index().saturating_sub(1);
    state.set_sender_chain(new_ratchet, sender_chain_key);
    let index = state
        .receiver_chain_index(their_ratchet)
        .ok_or(Error::InvalidState(
            "decrypt",
            "receiver chain was not stored",
        ))?;
    Ok((index, receiver_chain))
}

/// Returns the message keys for `counter` on the chain at `chain_index`, advancing the chain
/// and saving the keys of skipped messages.
fn get_or_create_message_keys(
    state: &mut SessionState,
    chain_index: usize,
    chain_key: ChainKey,
    counter: u32,
) -> Result<MessageKeys> {
    if chain_key.index() > counter {
        return state
            .take_message_keys(chain_index, counter)
            .ok_or(Error::DuplicatedMessage(chain_key.index(), counter));
    }
    if counter - chain_key.index() > MAX_FORWARD_JUMPS {
        return Err(Error::InvalidMessage(
            CiphertextMessageType::Whisper,
            "counter is too far ahead of the chain",
        ));
    }
    let mut current = chain_key;
    while current.index() < counter {
        state.save_message_keys(chain_index, current.message_keys()?);
        current = current.next()?;
    }
    let next = current.next()?;
    let keys = current.message_keys()?;
    if let Some(chain) = state.receiver_chains.get_mut(chain_index) {
        chain.chain_key = next;
    }
    Ok(keys)
}
