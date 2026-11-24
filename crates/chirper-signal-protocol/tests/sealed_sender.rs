//! Sealed sender v1 tests: round trip, certificate validation, tampering and recipient binding.

use chirper_signal_protocol::Error;
use chirper_signal_protocol::curve::KeyPair;
use chirper_signal_protocol::identity::IdentityKeyPair;
use chirper_signal_protocol::protocol::CiphertextMessageType;
use chirper_signal_protocol::sealed_sender::{
    SenderCertificate, ServerCertificate, UnidentifiedSenderMessageContent, decrypt,
    decrypt_to_content, encrypt,
};
use rand::SeedableRng;
use rand::rngs::StdRng;

const NOW_MS: u64 = 1_700_000_000_000;
const EXPIRES_MS: u64 = NOW_MS + 86_400_000;

struct World {
    trust_root: KeyPair,
    sender: IdentityKeyPair,
    recipient: IdentityKeyPair,
    sender_cert: SenderCertificate,
    rng: StdRng,
}

fn world(seed: u64) -> World {
    let mut rng = StdRng::seed_from_u64(seed);
    let trust_root = KeyPair::generate(&mut rng);
    let server = KeyPair::generate(&mut rng);
    let server_cert =
        ServerCertificate::new(7, server.public_key, &trust_root.private_key, &mut rng).unwrap();
    let sender = IdentityKeyPair::generate(&mut rng);
    let recipient = IdentityKeyPair::generate(&mut rng);
    let sender_cert = SenderCertificate::new(
        "11111111-2222-3333-4444-555555555555".to_owned(),
        Some("+15555550100".to_owned()),
        *sender.identity_key().public_key(),
        3,
        EXPIRES_MS,
        server_cert,
        &server.private_key,
        &mut rng,
    )
    .unwrap();
    World {
        trust_root,
        sender,
        recipient,
        sender_cert,
        rng,
    }
}

fn seal(w: &mut World, body: &[u8]) -> Vec<u8> {
    let content = UnidentifiedSenderMessageContent::new(
        CiphertextMessageType::Whisper,
        w.sender_cert.clone(),
        body.to_vec(),
        1,
        Some(b"group-9".to_vec()),
    )
    .unwrap();
    encrypt(&w.sender, &w.recipient.identity_key(), &content, &mut w.rng).unwrap()
}

#[test]
fn round_trip_recovers_sender_and_content() {
    let mut w = world(1);
    let sealed = seal(&mut w, b"inner ciphertext");
    assert_eq!(sealed[0], 0x11);
    let opened = decrypt(&w.recipient, &sealed, &w.trust_root.public_key, NOW_MS).unwrap();
    assert_eq!(opened.sender_uuid, "11111111-2222-3333-4444-555555555555");
    assert_eq!(opened.sender_e164.as_deref(), Some("+15555550100"));
    assert_eq!(opened.sender_device_id, 3);
    assert_eq!(opened.message_type, CiphertextMessageType::Whisper);
    assert_eq!(opened.contents, b"inner ciphertext");
    assert_eq!(opened.content_hint, 1);
    assert_eq!(opened.group_id.as_deref(), Some(&b"group-9"[..]));
}

#[test]
fn expired_certificate_is_rejected() {
    let mut w = world(3);
    let sealed = seal(&mut w, b"late");
    let result = decrypt(
        &w.recipient,
        &sealed,
        &w.trust_root.public_key,
        EXPIRES_MS + 1,
    );
    assert!(matches!(result, Err(Error::InvalidSealedSenderMessage(_))));
}

#[test]
fn certificate_from_another_trust_root_is_rejected() {
    let mut w = world(4);
    let sealed = seal(&mut w, b"hi");
    let other = KeyPair::generate(&mut StdRng::seed_from_u64(99));
    let result = decrypt(&w.recipient, &sealed, &other.public_key, NOW_MS);
    assert!(matches!(result, Err(Error::InvalidSealedSenderMessage(_))));
}

#[test]
fn only_the_intended_recipient_can_open_the_envelope() {
    let mut w = world(5);
    let sealed = seal(&mut w, b"private");
    let eve = IdentityKeyPair::generate(&mut StdRng::seed_from_u64(55));
    assert!(decrypt_to_content(&eve, &sealed).is_err());
    assert!(decrypt_to_content(&w.recipient, &sealed).is_ok());
}

#[test]
fn unknown_version_is_reported() {
    let mut w = world(6);
    let mut sealed = seal(&mut w, b"v");
    sealed[0] = 0x22;
    assert!(matches!(
        decrypt_to_content(&w.recipient, &sealed),
        Err(Error::UnknownSealedSenderVersion(2))
    ));
    assert!(matches!(
        decrypt_to_content(&w.recipient, &[]),
        Err(Error::InvalidSealedSenderMessage(_))
    ));
}

#[test]
fn every_truncation_and_bit_flip_fails_or_is_harmless() {
    let mut w = world(7);
    let sealed = seal(&mut w, b"integrity");
    for len in 0..sealed.len() {
        assert!(
            decrypt(
                &w.recipient,
                &sealed[..len],
                &w.trust_root.public_key,
                NOW_MS
            )
            .is_err(),
            "prefix {len}"
        );
    }
    // Flipping any bit of the sealed envelope must never yield a valid message: the MACs cover
    // the encrypted static key and the encrypted content.
    for i in 0..sealed.len() * 8 {
        let mut bad = sealed.clone();
        bad[i / 8] ^= 1 << (i % 8);
        let result = decrypt(&w.recipient, &bad, &w.trust_root.public_key, NOW_MS);
        assert!(result.is_err(), "bit {i} accepted");
    }
}

#[test]
fn server_certificate_round_trips_and_validates() {
    let mut rng = StdRng::seed_from_u64(8);
    let root = KeyPair::generate(&mut rng);
    let server = KeyPair::generate(&mut rng);
    let cert = ServerCertificate::new(42, server.public_key, &root.private_key, &mut rng).unwrap();
    let parsed = ServerCertificate::deserialize(cert.serialized()).unwrap();
    assert_eq!(parsed.key_id(), 42);
    assert_eq!(parsed.key(), &server.public_key);
    assert!(parsed.validate(&root.public_key));
    assert!(!parsed.validate(&server.public_key));
    assert!(ServerCertificate::deserialize(&cert.serialized()[..5]).is_err());
}
