//! Group (sender key) tests: three members, distribution, out-of-order delivery and rejection.

mod support;

use chirper_signal_protocol::Error;
use chirper_signal_protocol::group::{
    SenderKeyName, SenderKeyRecord, create_sender_key_distribution_message, group_decrypt,
    group_encrypt, process_sender_key_distribution_message,
};
use chirper_signal_protocol::message::{SenderKeyDistributionMessage, SenderKeyMessage};
use chirper_signal_protocol::store::SenderKeyStore;
use support::Device;

const GROUP: &str = "group-42";

fn name_of(device: &Device) -> SenderKeyName {
    SenderKeyName::new(GROUP, device.address.clone())
}

/// Each member creates its sending chain and every other member learns it, as a driver would
/// after fan-out of the distribution messages.
fn distribute(members: &mut [Device]) {
    for owner in 0..members.len() {
        let name = name_of(&members[owner]);
        let skdm = create_sender_key_distribution_message(
            &mut members[owner].store,
            &name,
            &mut members[owner].rng,
        )
        .unwrap();
        let wire = SenderKeyDistributionMessage::deserialize(skdm.serialized()).unwrap();
        for (other, member) in members.iter_mut().enumerate() {
            if other != owner {
                process_sender_key_distribution_message(&mut member.store, &name, &wire).unwrap();
            }
        }
    }
}

fn send(members: &mut [Device], from: usize, text: &str) -> SenderKeyMessage {
    let name = name_of(&members[from]);
    let member = &mut members[from];
    let message =
        group_encrypt(&mut member.store, &name, text.as_bytes(), &mut member.rng).unwrap();
    SenderKeyMessage::deserialize(message.serialized()).unwrap()
}

fn receive(
    members: &mut [Device],
    at: usize,
    from: usize,
    message: &SenderKeyMessage,
) -> Result<Vec<u8>, Error> {
    let name = name_of(&members[from]);
    group_decrypt(&mut members[at].store, &name, message)
}

fn three_members() -> Vec<Device> {
    let mut members = vec![
        Device::new("alice", 100),
        Device::new("bob", 101),
        Device::new("carol", 102),
    ];
    distribute(&mut members);
    members
}

#[test]
fn three_members_exchange_in_order() {
    let mut members = three_members();
    for round in 0..10 {
        let sender = round % 3;
        let text = format!("round {round} from {sender}");
        let message = send(&mut members, sender, &text);
        for receiver in 0..3 {
            if receiver != sender {
                assert_eq!(
                    receive(&mut members, receiver, sender, &message).unwrap(),
                    text.as_bytes()
                );
            }
        }
    }
}

#[test]
fn out_of_order_delivery_uses_saved_message_keys() {
    let mut members = three_members();
    let messages: Vec<SenderKeyMessage> = (0..10)
        .map(|i| send(&mut members, 0, &format!("m{i}")))
        .collect();
    // Carol receives the second half first, then the first half, and then a replay fails.
    for (i, message) in messages.iter().enumerate().skip(5) {
        assert_eq!(
            receive(&mut members, 2, 0, message).unwrap(),
            format!("m{i}").as_bytes()
        );
    }
    for (i, message) in messages.iter().enumerate().take(5) {
        assert_eq!(
            receive(&mut members, 2, 0, message).unwrap(),
            format!("m{i}").as_bytes()
        );
    }
    assert!(matches!(
        receive(&mut members, 2, 0, &messages[3]),
        Err(Error::DuplicatedMessage(..))
    ));
    // Bob, who saw everything in order, is unaffected.
    assert_eq!(receive(&mut members, 1, 0, &messages[9]).unwrap(), b"m9");
}

#[test]
fn forged_signature_is_rejected_and_state_is_unchanged() {
    let mut members = three_members();
    let good = send(&mut members, 0, "authentic");
    let mut bytes = good.serialized().to_vec();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    let forged = SenderKeyMessage::deserialize(&bytes).unwrap();
    assert!(matches!(
        receive(&mut members, 1, 0, &forged),
        Err(Error::SignatureValidationFailed)
    ));
    assert_eq!(receive(&mut members, 1, 0, &good).unwrap(), b"authentic");
}

#[test]
fn a_message_from_another_members_chain_is_rejected_for_this_sender() {
    let mut members = three_members();
    let from_bob = send(&mut members, 1, "from bob");
    assert!(
        receive(&mut members, 2, 0, &from_bob).is_err(),
        "bob's chain is not alice's"
    );
}

#[test]
fn unknown_sender_key_is_reported() {
    let mut members = three_members();
    let message = send(&mut members, 0, "hello");
    let stranger = SenderKeyName::new("other-group", members[0].address.clone());
    assert!(matches!(
        group_decrypt(&mut members[1].store, &stranger, &message),
        Err(Error::NoSenderKeyState)
    ));
}

#[test]
fn distribution_message_round_trips_and_a_learner_cannot_encrypt() {
    let mut alice = Device::new("alice", 110);
    let name = name_of(&alice);
    let skdm =
        create_sender_key_distribution_message(&mut alice.store, &name, &mut alice.rng).unwrap();
    let parsed = SenderKeyDistributionMessage::deserialize(skdm.serialized()).unwrap();
    assert_eq!(parsed, skdm);
    assert_eq!(parsed.iteration(), 0);

    // A learner stores a remote state, which can decrypt but cannot encrypt.
    let mut bob = Device::new("bob", 111);
    process_sender_key_distribution_message(&mut bob.store, &name, &parsed).unwrap();
    assert!(matches!(
        chirper_signal_protocol::group::group_encrypt(&mut bob.store, &name, b"nope", &mut bob.rng),
        Err(Error::InvalidSenderKeySession)
    ));
}

#[test]
fn sender_key_record_survives_serialization_mid_conversation() {
    let mut members = three_members();
    let first = send(&mut members, 0, "one");
    assert_eq!(receive(&mut members, 1, 0, &first).unwrap(), b"one");

    let name = name_of(&members[0]);
    let saved = members[1]
        .store
        .load_sender_key(&name)
        .unwrap()
        .unwrap()
        .serialize();
    let restored = SenderKeyRecord::deserialize(&saved).unwrap();
    assert_eq!(restored.serialize(), saved);
    let name = name_of(&members[0]);
    members[1].store.store_sender_key(&name, &restored).unwrap();

    let second = send(&mut members, 0, "two");
    assert_eq!(receive(&mut members, 1, 0, &second).unwrap(), b"two");
}

#[test]
fn group_message_parsers_reject_truncation_and_never_panic() {
    let mut members = three_members();
    let message = send(&mut members, 0, "parse me");
    let bytes = message.serialized().to_vec();
    for len in 0..bytes.len() {
        assert!(
            SenderKeyMessage::deserialize(&bytes[..len]).is_err(),
            "prefix {len} parsed"
        );
    }
    for i in 0..bytes.len() * 8 {
        let mut bad = bytes.clone();
        bad[i / 8] ^= 1 << (i % 8);
        if let Ok(parsed) = SenderKeyMessage::deserialize(&bad) {
            let _ = receive(&mut members, 1, 0, &parsed);
        }
    }
    assert_eq!(receive(&mut members, 1, 0, &message).unwrap(), b"parse me");
}
