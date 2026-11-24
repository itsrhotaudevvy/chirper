//! One-to-one session tests: the libsignal Go scenarios, PQXDH, archiving, rejection and parsing.

mod support;

use chirper_signal_protocol::Error;
use chirper_signal_protocol::cipher::{decrypt_signal_message, encrypt};
use chirper_signal_protocol::message::{CiphertextMessage, PreKeySignalMessage, SignalMessage};
use chirper_signal_protocol::session::SessionRecord;
use chirper_signal_protocol::session_builder::process_prekey_bundle;
use chirper_signal_protocol::store::{IdentityKeyStore, SessionStore};
use support::{Device, establish, is_duplicate, over_the_wire, receive, send};

fn texts(count: usize) -> Vec<String> {
    let lorem = [
        "Lorem ipsum dolor sit amet, consectetur adipiscing elit.",
        "Ut enim ad minim veniam, quis nostrud exercitation.",
        "Duis aute irure dolor in reprehenderit in voluptate.",
    ];
    (0..count)
        .map(|i| format!("{i}: {}", lorem[i % lorem.len()]))
        .collect()
}

/// Sends every text and checks that the receiver gets each one back in order.
fn exchange(sender: &mut Device, receiver: &mut Device, count: usize) {
    for text in texts(count) {
        let wire = send(sender, receiver, text.as_bytes());
        assert_eq!(receive(receiver, sender, &wire).unwrap(), text.as_bytes());
    }
}

#[test]
fn basic_session_both_directions_and_wire_version() {
    let mut alice = Device::new("alice", 1);
    let mut bob = Device::new("bob", 2);
    establish(&mut alice, &mut bob, false);

    let first = encrypt(&mut alice.store, &bob.address, b"Hello!").unwrap();
    assert!(matches!(first, CiphertextMessage::PreKey(_)));
    assert_eq!(
        first.serialized()[0],
        b'3',
        "version 3 messages start with '3'"
    );
    let wire = over_the_wire(&first);
    assert_eq!(receive(&mut bob, &alice, &wire).unwrap(), b"Hello!");

    let reply = send(&mut bob, &alice, b"oui!");
    assert!(
        matches!(reply, CiphertextMessage::Signal(_)),
        "after a reply, messages are Signal messages"
    );
    assert_eq!(receive(&mut alice, &bob, &reply).unwrap(), b"oui!");
}

#[test]
fn roundtrip_100_messages_each_way_over_three_rounds() {
    let mut alice = Device::new("alice", 3);
    let mut bob = Device::new("bob", 4);
    establish(&mut alice, &mut bob, false);
    for _ in 0..3 {
        exchange(&mut alice, &mut bob, 100);
        exchange(&mut bob, &mut alice, 100);
    }
}

#[test]
fn out_of_order_and_dropped_messages() {
    let mut alice = Device::new("alice", 5);
    let mut bob = Device::new("bob", 6);
    establish(&mut alice, &mut bob, false);
    let wires: Vec<CiphertextMessage> = (0..6)
        .map(|i| send(&mut alice, &bob, format!("m{i}").as_bytes()))
        .collect();

    // Message 3 arrives first; the prekey message (0) is then replayed late, as in the Go test.
    assert_eq!(receive(&mut bob, &alice, &wires[3]).unwrap(), b"m3");
    assert_eq!(receive(&mut bob, &alice, &wires[0]).unwrap(), b"m0");
    assert_eq!(receive(&mut bob, &alice, &wires[2]).unwrap(), b"m2");
    // Message 1 is never delivered by the network: it is simply skipped over.
    assert_eq!(receive(&mut bob, &alice, &wires[5]).unwrap(), b"m5");
    // A message dropped forever does not block the rest.
    assert_eq!(receive(&mut bob, &alice, &wires[4]).unwrap(), b"m4");
    // Replaying an already decrypted message is a duplicate.
    assert!(is_duplicate(&receive(&mut bob, &alice, &wires[4])));
}

#[test]
fn message_key_limit_drops_the_oldest_saved_keys() {
    let mut alice = Device::new("alice", 7);
    let mut bob = Device::new("bob", 8);
    establish(&mut alice, &mut bob, false);
    let wires: Vec<CiphertextMessage> = (0..2010)
        .map(|i| send(&mut alice, &bob, format!("{i}").as_bytes()))
        .collect();

    assert_eq!(receive(&mut bob, &alice, &wires[0]).unwrap(), b"0");
    // Jumping to counter 2009 saves keys for 1..=2008, but only the newest 2000 are kept.
    assert_eq!(receive(&mut bob, &alice, &wires[2009]).unwrap(), b"2009");
    assert!(
        is_duplicate(&receive(&mut bob, &alice, &wires[1])),
        "key 1 was evicted"
    );
    assert_eq!(
        receive(&mut bob, &alice, &wires[9]).unwrap(),
        b"9",
        "key 9 is still saved"
    );
    assert_eq!(receive(&mut bob, &alice, &wires[2008]).unwrap(), b"2008");
}

#[test]
fn forward_jump_beyond_the_cap_is_rejected_without_side_effects() {
    let mut alice = Device::new("alice", 9);
    let mut bob = Device::new("bob", 10);
    establish(&mut alice, &mut bob, false);
    // Counter 0 is the prekey message; counters 1..=25_002 follow.
    let wires: Vec<CiphertextMessage> = (0..25_003)
        .map(|_| send(&mut alice, &bob, b"filler"))
        .collect();
    assert_eq!(receive(&mut bob, &alice, &wires[0]).unwrap(), b"filler");
    // From chain index 1, counter 25_002 is a jump of 25_001, beyond the cap of 25_000.
    assert!(receive(&mut bob, &alice, &wires[25_002]).is_err());
    // The failed attempt committed nothing, so the next message in order still decrypts.
    assert_eq!(receive(&mut bob, &alice, &wires[1]).unwrap(), b"filler");
}

#[test]
fn prekey_reuse_is_detected_and_one_time_key_is_consumed() {
    let mut alice = Device::new("alice", 11);
    let mut bob = Device::new("bob", 12);
    let bundle = bob.publish_bundle(false);
    let one_time_id = bundle.pre_key.as_ref().unwrap().0;
    process_prekey_bundle(&mut alice.store, &bob.address, &bundle, &mut alice.rng).unwrap();
    let wire = send(&mut alice, &bob, b"once");

    assert_eq!(receive(&mut bob, &alice, &wire).unwrap(), b"once");
    assert!(
        !bob.store.contains_pre_key(one_time_id),
        "one-time prekey must be removed"
    );
    assert!(
        is_duplicate(&receive(&mut bob, &alice, &wire)),
        "a replayed prekey message is a duplicate"
    );
}

#[test]
fn simultaneous_initiation_both_sides_send_prekey_messages() {
    let mut alice = Device::new("alice", 13);
    let mut bob = Device::new("bob", 14);
    let for_bob = bob.publish_bundle(false);
    let for_alice = alice.publish_bundle(false);
    process_prekey_bundle(&mut alice.store, &bob.address, &for_bob, &mut alice.rng).unwrap();
    process_prekey_bundle(&mut bob.store, &alice.address, &for_alice, &mut bob.rng).unwrap();

    let from_alice = send(&mut alice, &bob, b"hi from alice");
    let from_bob = send(&mut bob, &alice, b"hi from bob");
    assert!(matches!(from_alice, CiphertextMessage::PreKey(_)));
    assert!(matches!(from_bob, CiphertextMessage::PreKey(_)));
    assert_eq!(
        receive(&mut bob, &alice, &from_alice).unwrap(),
        b"hi from alice"
    );
    assert_eq!(
        receive(&mut alice, &bob, &from_bob).unwrap(),
        b"hi from bob"
    );
    exchange(&mut alice, &mut bob, 5);
    exchange(&mut bob, &mut alice, 5);
}

#[test]
fn new_prekey_message_archives_the_old_session_which_stays_readable() {
    let mut alice = Device::new("alice", 15);
    let mut bob = Device::new("bob", 16);
    establish(&mut alice, &mut bob, false);
    let old_prekey = send(&mut alice, &bob, b"session one");
    assert_eq!(
        receive(&mut bob, &alice, &old_prekey).unwrap(),
        b"session one"
    );
    let old_reply = send(&mut bob, &alice, b"ack one");
    assert_eq!(receive(&mut alice, &bob, &old_reply).unwrap(), b"ack one");
    let old_later = send(&mut alice, &bob, b"still session one");

    // Alice starts a second session with a fresh prekey bundle.
    establish(&mut alice, &mut bob, false);
    let new_prekey = send(&mut alice, &bob, b"session two");
    assert!(matches!(new_prekey, CiphertextMessage::PreKey(_)));
    assert_eq!(
        receive(&mut bob, &alice, &new_prekey).unwrap(),
        b"session two"
    );
    exchange(&mut bob, &mut alice, 3);
    exchange(&mut alice, &mut bob, 3);

    // A message from the archived session still decrypts.
    assert_eq!(
        receive(&mut bob, &alice, &old_later).unwrap(),
        b"still session one"
    );
}

#[test]
fn untrusted_identity_is_rejected_in_both_directions() {
    let mut alice = Device::new("alice", 17);
    let mut bob = Device::new("bob", 18);
    let mut mallory = Device::new("mallory", 19);
    establish(&mut alice, &mut bob, false);
    let wire = send(&mut alice, &bob, b"hi");
    assert_eq!(receive(&mut bob, &alice, &wire).unwrap(), b"hi");

    // Bob learns a different identity for Alice's address, so Alice's next prekey message is refused.
    let impostor = mallory.store.identity_key_pair().unwrap().identity_key();
    bob.store.save_identity(&alice.address, &impostor).unwrap();
    establish(&mut alice, &mut bob, false);
    let fresh = send(&mut alice, &bob, b"new session");
    assert!(matches!(
        receive(&mut bob, &alice, &fresh),
        Err(Error::UntrustedIdentity(_))
    ));

    // Alice refuses a bundle whose identity differs from the one she already trusts for Bob.
    let bundle = mallory.publish_bundle(false);
    alice
        .store
        .save_identity(
            &bob.address,
            &bob.store.identity_key_pair().unwrap().identity_key(),
        )
        .unwrap();
    let result = process_prekey_bundle(&mut alice.store, &bob.address, &bundle, &mut alice.rng);
    assert!(matches!(result, Err(Error::UntrustedIdentity(_))));
}

#[test]
fn tampered_message_is_rejected_and_leaves_the_session_intact() {
    let mut alice = Device::new("alice", 20);
    let mut bob = Device::new("bob", 21);
    establish(&mut alice, &mut bob, false);
    let good = send(&mut alice, &bob, b"good");
    assert_eq!(receive(&mut bob, &alice, &good).unwrap(), b"good");

    let next = encrypt(&mut alice.store, &bob.address, b"tampered").unwrap();
    let mut bytes = next.serialized().to_vec();
    // Flip a byte inside the ciphertext, which sits just before the 8-byte MAC.
    let flip = bytes.len() - 12;
    bytes[flip] ^= 0x01;
    let CiphertextMessage::PreKey(_) = &next else {
        panic!("expected a prekey message before alice hears back");
    };
    let tampered = CiphertextMessage::PreKey(PreKeySignalMessage::deserialize(&bytes).unwrap());
    assert!(matches!(
        receive(&mut bob, &alice, &tampered),
        Err(Error::InvalidMessage(..))
    ));
    // The same message, untampered, still decrypts: the failed attempt changed nothing.
    assert_eq!(
        receive(&mut bob, &alice, &over_the_wire(&next)).unwrap(),
        b"tampered"
    );
}

#[test]
fn version_4_pqxdh_session_uses_kyber_and_consumes_the_kyber_prekey() {
    let mut alice = Device::new("alice", 22);
    let mut bob = Device::new("bob", 23);
    let bundle = bob.publish_bundle(true);
    let kyber_id = bundle.kyber_pre_key.as_ref().unwrap().id;
    process_prekey_bundle(&mut alice.store, &bob.address, &bundle, &mut alice.rng).unwrap();

    let first = encrypt(&mut alice.store, &bob.address, b"pq hello").unwrap();
    assert_eq!(
        first.serialized()[0],
        0x44,
        "version 4 messages start with 0x44"
    );
    let CiphertextMessage::PreKey(pre) = &first else {
        panic!("first message must be a prekey message");
    };
    assert_eq!(pre.kyber_pre_key_id(), Some(kyber_id));
    assert_eq!(pre.kyber_ciphertext().unwrap().len(), 1569);

    let wire = over_the_wire(&first);
    assert_eq!(receive(&mut bob, &alice, &wire).unwrap(), b"pq hello");
    assert!(
        !bob.store.contains_kyber_pre_key(kyber_id),
        "kyber prekey must be marked used"
    );
    exchange(&mut bob, &mut alice, 3);
    exchange(&mut alice, &mut bob, 3);
}

#[test]
fn a_bundle_without_kyber_gives_version_3() {
    let mut alice = Device::new("alice", 24);
    let mut bob = Device::new("bob", 25);
    establish(&mut alice, &mut bob, false);
    let first = encrypt(&mut alice.store, &bob.address, b"x").unwrap();
    assert_eq!(first.serialized()[0], b'3');
}

#[test]
fn bundle_with_a_forged_kyber_signature_is_rejected() {
    let mut alice = Device::new("alice", 26);
    let mut bob = Device::new("bob", 27);
    let mut bundle = bob.publish_bundle(true);
    if let Some(kyber) = bundle.kyber_pre_key.as_mut() {
        kyber.signature[0] ^= 0x80;
    }
    let result = process_prekey_bundle(&mut alice.store, &bob.address, &bundle, &mut alice.rng);
    assert!(matches!(result, Err(Error::SignatureValidationFailed)));
}

#[test]
fn bundle_with_a_forged_signed_prekey_is_rejected() {
    let mut alice = Device::new("alice", 28);
    let mut bob = Device::new("bob", 29);
    let mut bundle = bob.publish_bundle(false);
    bundle.signed_pre_key_signature[5] ^= 0x01;
    let result = process_prekey_bundle(&mut alice.store, &bob.address, &bundle, &mut alice.rng);
    assert!(matches!(result, Err(Error::SignatureValidationFailed)));
}

#[test]
fn session_record_survives_serialization_mid_conversation() {
    let mut alice = Device::new("alice", 30);
    let mut bob = Device::new("bob", 31);
    establish(&mut alice, &mut bob, false);
    exchange(&mut alice, &mut bob, 4);
    let wire = send(&mut bob, &alice, b"before restart");
    assert_eq!(receive(&mut alice, &bob, &wire).unwrap(), b"before restart");

    // Bob's record is written out and read back into a fresh store, as a driver would.
    let saved = bob
        .store
        .load_session(&alice.address)
        .unwrap()
        .unwrap()
        .serialize();
    let restored = SessionRecord::deserialize(&saved).unwrap();
    assert_eq!(restored.serialize(), saved, "serialization is stable");
    bob.store.store_session(&alice.address, &restored).unwrap();

    exchange(&mut alice, &mut bob, 4);
    exchange(&mut bob, &mut alice, 4);
}

#[test]
fn truncated_prekey_messages_fail_to_parse() {
    let mut alice = Device::new("alice", 32);
    let mut bob = Device::new("bob", 33);
    let bundle = bob.publish_bundle(true);
    process_prekey_bundle(&mut alice.store, &bob.address, &bundle, &mut alice.rng).unwrap();
    let message = encrypt(&mut alice.store, &bob.address, b"fuzz me").unwrap();
    let bytes = message.serialized().to_vec();
    assert!(PreKeySignalMessage::deserialize(&bytes).is_ok());
    for len in 0..bytes.len() {
        assert!(
            PreKeySignalMessage::deserialize(&bytes[..len]).is_err(),
            "prefix {len} parsed"
        );
    }
    for i in 0..bytes.len() * 8 {
        let mut bad = bytes.clone();
        bad[i / 8] ^= 1 << (i % 8);
        let _ = PreKeySignalMessage::deserialize(&bad);
    }
}

#[test]
fn signal_message_bit_flips_never_decrypt_and_never_panic() {
    let mut alice = Device::new("alice", 34);
    let mut bob = Device::new("bob", 35);
    establish(&mut alice, &mut bob, false);
    let opening = send(&mut alice, &bob, b"open");
    assert_eq!(receive(&mut bob, &alice, &opening).unwrap(), b"open");
    let reply = send(&mut bob, &alice, b"reply");
    assert_eq!(receive(&mut alice, &bob, &reply).unwrap(), b"reply");

    let original = encrypt(&mut alice.store, &bob.address, b"signal payload").unwrap();
    let CiphertextMessage::Signal(_) = &original else {
        panic!("expected a Signal message after a reply");
    };
    let bytes = original.serialized().to_vec();
    for len in 0..bytes.len() {
        assert!(
            SignalMessage::deserialize(&bytes[..len]).is_err(),
            "prefix {len} parsed"
        );
    }
    for i in 0..bytes.len() * 8 {
        let mut bad = bytes.clone();
        bad[i / 8] ^= 1 << (i % 8);
        if let Ok(parsed) = SignalMessage::deserialize(&bad) {
            let result =
                decrypt_signal_message(&mut bob.store, &alice.address, &parsed, &mut bob.rng);
            assert!(result.is_err(), "bit {i} decrypted");
        }
    }
    // The untouched message still decrypts after all those failed attempts.
    assert_eq!(
        receive(&mut bob, &alice, &over_the_wire(&original)).unwrap(),
        b"signal payload"
    );
}
