//! Session establishment: Alice processes a prekey bundle, Bob processes a prekey message.
//!
//! Port of libsignal `session/Session.go` (`Builder.ProcessBundle`, `Builder.processV3`) with
//! the PQXDH branch of the Rust implementation (AGPL-3.0-only, Signal Messenger LLC).

use rand::{CryptoRng, RngCore};
use zeroize::Zeroizing;

use crate::ProtocolAddress;
use crate::curve::KeyPair;
use crate::error::{Error, Result};
use crate::message::PreKeySignalMessage;
use crate::prekey::{KyberPreKeyRecord, PreKeyBundle};
use crate::ratchet::{self, AliceParameters, BobParameters};
use crate::session::{PendingKyberMessage, PendingPreKeyMessage, SessionRecord, SessionState};
use crate::store::ProtocolStore;

/// What Bob's side consumed while accepting a prekey message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreKeyOutcome {
    /// The one-time prekey used, which the caller must delete after a successful decryption.
    pub pre_key_id: Option<u32>,
    /// The Kyber prekey used, which the caller must mark used after a successful decryption.
    pub kyber_pre_key_id: Option<u32>,
}

/// Alice's side: verifies `bundle` and installs a new session with `address`, then stores the
/// session and the peer's identity.
///
/// Signatures are checked before anything is stored. The session version is 4 when the bundle
/// has a Kyber prekey and 3 otherwise. An untrusted identity fails with
/// [`Error::UntrustedIdentity`].
pub fn process_prekey_bundle<S, R>(
    store: &mut S,
    address: &ProtocolAddress,
    bundle: &PreKeyBundle,
    csprng: &mut R,
) -> Result<()>
where
    S: ProtocolStore + ?Sized,
    R: CryptoRng + RngCore,
{
    if !store.is_trusted_identity(address, &bundle.identity_key)? {
        return Err(Error::UntrustedIdentity(address.clone()));
    }
    let identity = bundle.identity_key.public_key();
    if !identity.verify_signature(
        &bundle.signed_pre_key_public.serialize(),
        &bundle.signed_pre_key_signature,
    ) {
        return Err(Error::SignatureValidationFailed);
    }
    if bundle.kyber_pre_key.as_ref().is_some_and(|kyber| {
        !identity.verify_signature(&kyber.public_key.serialize(), &kyber.signature)
    }) {
        return Err(Error::SignatureValidationFailed);
    }

    let our_identity = store.identity_key_pair()?;
    let our_base = KeyPair::generate(csprng);
    let kyber_secret = bundle
        .kyber_pre_key
        .as_ref()
        .map(|kyber| kyber.public_key.encapsulate(csprng))
        .transpose()?;
    let kyber_shared = kyber_secret.as_ref().map(|(secret, _)| secret.as_slice());
    let version = ratchet::version_for(kyber_shared);

    let keys = ratchet::initialize_alice_session(
        &AliceParameters {
            our_identity: &our_identity,
            our_base_key: &our_base,
            their_identity: &bundle.identity_key,
            their_signed_pre_key: &bundle.signed_pre_key_public,
            their_one_time_pre_key: bundle.pre_key.as_ref().map(|(_, key)| key),
            kyber_shared_secret: kyber_shared,
        },
        version,
    )?;
    let sending_ratchet = KeyPair::generate(csprng);
    let (root_key, sending_chain_key) = keys
        .root_key
        .create_chain(&bundle.signed_pre_key_public, &sending_ratchet)?;

    let mut state = SessionState::new(
        version,
        our_identity.identity_key(),
        bundle.identity_key.clone(),
        root_key,
        store.local_registration_id()?,
        bundle.registration_id,
    );
    state.add_receiver_chain(bundle.signed_pre_key_public, keys.chain_key);
    state.set_sender_chain(sending_ratchet, sending_chain_key);
    state.alice_base_key = Some(our_base.public_key);
    state.set_pending_pre_key(PendingPreKeyMessage {
        pre_key_id: bundle.pre_key.as_ref().map(|(id, _)| *id),
        signed_pre_key_id: bundle.signed_pre_key_id,
        base_key: our_base.public_key,
    });
    if let (Some(kyber), Some((_, ciphertext))) = (&bundle.kyber_pre_key, kyber_secret) {
        state.set_pending_kyber(PendingKyberMessage {
            kyber_pre_key_id: kyber.id,
            ciphertext: ciphertext.into_vec(),
        });
    }

    let mut record = store.load_session(address)?.unwrap_or_default();
    record.archive_current_state();
    record.set_state(state);
    store.save_identity(address, &bundle.identity_key)?;
    store.store_session(address, &record)?;
    Ok(())
}

/// Bob's side: if `message` starts a new session, installs it as the current state of `record`.
///
/// Nothing is written to the store. The caller decrypts with `record`, and commits the record,
/// the identity and the consumed prekeys only if decryption succeeds. If `record` already has a
/// state for this message's base key, the message belongs to that state and nothing changes.
pub fn process_prekey_message<S>(
    store: &S,
    record: &mut SessionRecord,
    address: &ProtocolAddress,
    message: &PreKeySignalMessage,
) -> Result<PreKeyOutcome>
where
    S: ProtocolStore + ?Sized,
{
    let none = PreKeyOutcome {
        pre_key_id: None,
        kyber_pre_key_id: None,
    };
    if !store.is_trusted_identity(address, message.identity_key())? {
        return Err(Error::UntrustedIdentity(address.clone()));
    }
    if record.has_session_state(message.message_version(), message.base_key()) {
        return Ok(none);
    }

    let signed = store
        .load_signed_pre_key(message.signed_pre_key_id())?
        .ok_or(Error::InvalidSignedPreKeyId)?;
    let one_time = match message.pre_key_id() {
        Some(id) => Some(store.load_pre_key(id)?.ok_or(Error::InvalidPreKeyId)?),
        None => None,
    };
    let kyber_secret = match (message.kyber_pre_key_id(), message.kyber_ciphertext()) {
        (Some(id), Some(ciphertext)) => {
            let stored = store
                .load_kyber_pre_key(id)?
                .ok_or(Error::InvalidKyberPreKeyId)?;
            Some((id, decapsulate(&stored, ciphertext)?))
        }
        _ => None,
    };
    let kyber_shared = kyber_secret.as_ref().map(|(_, secret)| secret.as_slice());
    let our_identity = store.identity_key_pair()?;

    let keys = ratchet::initialize_bob_session(
        &BobParameters {
            our_identity: &our_identity,
            our_signed_pre_key: signed.key_pair(),
            our_one_time_pre_key: one_time.as_ref().map(|k| k.key_pair()),
            their_identity: message.identity_key(),
            their_base_key: message.base_key(),
            kyber_shared_secret: kyber_shared,
        },
        message.message_version(),
    )?;

    let mut state = SessionState::new(
        message.message_version(),
        our_identity.identity_key(),
        message.identity_key().clone(),
        keys.root_key,
        store.local_registration_id()?,
        message.registration_id(),
    );
    state.set_sender_chain(signed.key_pair().clone(), keys.chain_key);
    state.alice_base_key = Some(*message.base_key());

    record.archive_current_state();
    record.set_state(state);
    Ok(PreKeyOutcome {
        pre_key_id: message.pre_key_id(),
        kyber_pre_key_id: kyber_secret.map(|(id, _)| id),
    })
}

/// Decapsulates the Kyber ciphertext with the stored secret key.
fn decapsulate(record: &KyberPreKeyRecord, ciphertext: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    record.key_pair().secret_key.decapsulate(ciphertext)
}
