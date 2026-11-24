//! Wire interoperability with the Go libsignal that WhatsApp uses (`go.mau.fi/libsignal`).
//!
//! The vectors in `tests/interop/go_vectors.json` come from `tests/interop/generate`, a Go program
//! that drives libsignal with fixed keys. These tests rebuild Bob's stores from the recorded
//! private keys, decrypt every Go-produced message with this crate, and check the plaintexts.
//! Messages after the first one are decrypted from libsignal's own session records, which the
//! generator captured just before each receiver decrypted them. The reverse direction (Rust
//! replies that Go decrypts) is checked by `examples/interop_reply.rs` and the Go `verify` mode.
//! Nothing here runs Go.

#[path = "interop/vectors.rs"]
mod vectors;

use chirper_signal_protocol::Error;
use chirper_signal_protocol::cipher::{decrypt_prekey_message, decrypt_signal_message};
use chirper_signal_protocol::group::{
    SenderKeyName, group_decrypt, process_sender_key_distribution_message,
};
use chirper_signal_protocol::message::{
    PreKeySignalMessage, SenderKeyDistributionMessage, SenderKeyMessage, SignalMessage,
};
use chirper_signal_protocol::session::SessionRecord;
use rand::SeedableRng;
use rand::rngs::StdRng;
use vectors::{
    BoxError, alice_address, bob_address, bob_store, bytes, empty_store, load,
    load_receiver_session, message,
};

fn rng() -> StdRng {
    StdRng::seed_from_u64(7)
}

/// Decrypts Go message `index` with a store for the receiver, starting from the session record
/// Go captured for it, and checks the plaintext.
fn decrypt_go_signal(index: usize, receiver_is_bob: bool) -> Result<(), BoxError> {
    let vectors = load()?;
    let go = message(&vectors, index)?;
    let (mut store, sender) = if receiver_is_bob {
        (bob_store(&vectors)?, alice_address(&vectors))
    } else {
        (empty_store(&vectors.alice)?, bob_address(&vectors))
    };
    load_receiver_session(&mut store, &sender, go)?;
    let wire = SignalMessage::deserialize(&bytes(&go.ciphertext)?)?;
    let plain = decrypt_signal_message(&mut store, &sender, &wire, &mut rng())?;
    assert_eq!(plain, go.plaintext.as_bytes(), "message {index}");
    Ok(())
}

#[test]
fn go_vectors_private_keys_derive_their_public_keys() -> Result<(), BoxError> {
    let vectors = load()?;
    assert!(vectors.library.starts_with("go.mau.fi/libsignal "));
    // bob_store checks each private key against its public key.
    bob_store(&vectors)?;
    empty_store(&vectors.alice)?;
    Ok(())
}

#[test]
fn go_prekey_message_decrypts_with_bobs_recreated_keys() -> Result<(), BoxError> {
    let vectors = load()?;
    let go = message(&vectors, 0)?;
    let mut store = bob_store(&vectors)?;
    let wire = PreKeySignalMessage::deserialize(&bytes(&go.ciphertext)?)?;
    assert_eq!(wire.message_version(), 3, "Go's session is X3DH, version 3");
    assert_eq!(wire.registration_id(), vectors.alice.registration_id);
    assert_eq!(wire.pre_key_id(), Some(vectors.bob.pre_key_id));
    assert_eq!(wire.signed_pre_key_id(), vectors.bob.signed_pre_key_id);

    let plain = decrypt_prekey_message(&mut store, &alice_address(&vectors), &wire, &mut rng())?;
    assert_eq!(plain, go.plaintext.as_bytes());
    assert!(
        !store.contains_pre_key(vectors.bob.pre_key_id),
        "one-time prekey consumed"
    );
    Ok(())
}

#[test]
fn go_reply_after_prekey_message_decrypts_on_alice() -> Result<(), BoxError> {
    decrypt_go_signal(1, false)
}

#[test]
fn go_follow_up_after_ratchet_step_decrypts_on_bob() -> Result<(), BoxError> {
    decrypt_go_signal(2, true)
}

#[test]
fn go_second_reply_after_ratchet_step_decrypts_on_alice() -> Result<(), BoxError> {
    decrypt_go_signal(3, false)
}

#[test]
fn go_replayed_message_is_a_duplicate() -> Result<(), BoxError> {
    let vectors = load()?;
    let go = message(&vectors, 2)?;
    let alice = alice_address(&vectors);
    let mut store = bob_store(&vectors)?;
    load_receiver_session(&mut store, &alice, go)?;
    let wire = SignalMessage::deserialize(&bytes(&go.ciphertext)?)?;
    decrypt_signal_message(&mut store, &alice, &wire, &mut rng())?;
    match decrypt_signal_message(&mut store, &alice, &wire, &mut rng()) {
        Err(Error::DuplicatedMessage(..)) => Ok(()),
        other => Err(format!("replay was not rejected as a duplicate: {other:?}").into()),
    }
}

#[test]
fn go_message_with_a_flipped_bit_is_rejected_and_leaves_the_session_intact() -> Result<(), BoxError>
{
    let vectors = load()?;
    let go = message(&vectors, 2)?;
    let alice = alice_address(&vectors);
    let clean = bytes(&go.ciphertext)?;
    let mut tampered = clean.clone();
    let middle = tampered.len() / 2;
    tampered[middle] ^= 0x01;

    let mut store = bob_store(&vectors)?;
    load_receiver_session(&mut store, &alice, go)?;
    let bad = SignalMessage::deserialize(&tampered)?;
    assert!(decrypt_signal_message(&mut store, &alice, &bad, &mut rng()).is_err());

    // The failed attempt committed nothing, so the untouched message still decrypts.
    let good = SignalMessage::deserialize(&clean)?;
    assert_eq!(
        decrypt_signal_message(&mut store, &alice, &good, &mut rng())?,
        go.plaintext.as_bytes()
    );
    Ok(())
}

#[test]
fn go_session_snapshots_parse_and_re_encode_stably() -> Result<(), BoxError> {
    // Go's encoder omits proto2 fields that hold their default (needsRefresh = false), and Rust
    // writes them, so the bytes differ. The check is that Rust reads the record as version 3
    // and that its own encoding is a fixed point.
    let vectors = load()?;
    for go in vectors
        .messages
        .iter()
        .filter_map(|m| m.receiver_session.as_deref())
    {
        let record = SessionRecord::deserialize(&bytes(go)?)?;
        assert_eq!(record.state().map(|state| state.version()), Some(3));
        let once = record.serialize();
        let twice = SessionRecord::deserialize(&once)?.serialize();
        assert_eq!(once.as_slice(), twice.as_slice());
    }
    Ok(())
}

#[test]
fn go_sender_key_messages_decrypt_out_of_order() -> Result<(), BoxError> {
    let vectors = load()?;
    let name = SenderKeyName::new(vectors.group.group_id.clone(), alice_address(&vectors));
    let distribution =
        SenderKeyDistributionMessage::deserialize(&bytes(&vectors.group.distribution_message)?)?;
    let first = SenderKeyMessage::deserialize(&bytes(&vectors.group.messages[0].ciphertext)?)?;
    let second = SenderKeyMessage::deserialize(&bytes(&vectors.group.messages[1].ciphertext)?)?;

    let mut store = empty_store(&vectors.bob.party)?;
    process_sender_key_distribution_message(&mut store, &name, &distribution)?;
    // The second message first forces the receiver to keep the skipped iteration-0 key.
    let late = group_decrypt(&mut store, &name, &second)?;
    let early = group_decrypt(&mut store, &name, &first)?;
    assert_eq!(late, vectors.group.messages[1].plaintext.as_bytes());
    assert_eq!(early, vectors.group.messages[0].plaintext.as_bytes());
    Ok(())
}

#[test]
fn go_sender_key_messages_decrypt_in_order() -> Result<(), BoxError> {
    let vectors = load()?;
    let name = SenderKeyName::new(vectors.group.group_id.clone(), alice_address(&vectors));
    let distribution =
        SenderKeyDistributionMessage::deserialize(&bytes(&vectors.group.distribution_message)?)?;
    let mut store = empty_store(&vectors.bob.party)?;
    process_sender_key_distribution_message(&mut store, &name, &distribution)?;
    for go in &vectors.group.messages {
        let wire = SenderKeyMessage::deserialize(&bytes(&go.ciphertext)?)?;
        assert_eq!(
            group_decrypt(&mut store, &name, &wire)?,
            go.plaintext.as_bytes()
        );
    }
    Ok(())
}
