//! Helpers shared by the integration tests: simulated devices, bundles and a wire hop.
#![allow(dead_code)]

use chirper_signal_protocol::cipher::{decrypt_prekey_message, decrypt_signal_message, encrypt};
use chirper_signal_protocol::identity::IdentityKeyPair;
use chirper_signal_protocol::memory::InMemoryProtocolStore;
use chirper_signal_protocol::message::{CiphertextMessage, PreKeySignalMessage, SignalMessage};
use chirper_signal_protocol::prekey::{
    KyberPreKeyBundle, PreKeyBundle, generate_kyber_pre_key, generate_pre_keys,
    generate_registration_id, generate_signed_pre_key,
};
use chirper_signal_protocol::session_builder::process_prekey_bundle;
use chirper_signal_protocol::store::{
    IdentityKeyStore, KyberPreKeyStore, PreKeyStore, SignedPreKeyStore,
};
use chirper_signal_protocol::{Error, ProtocolAddress, Result};
use rand::SeedableRng;
use rand::rngs::StdRng;

/// A simulated device: an address, a store, a seeded RNG and its own bundle keys.
pub struct Device {
    pub address: ProtocolAddress,
    pub store: InMemoryProtocolStore,
    pub registration_id: u32,
    pub rng: StdRng,
}

impl Device {
    /// A device with a deterministic identity derived from `seed`.
    pub fn new(name: &str, seed: u64) -> Self {
        let mut rng = StdRng::seed_from_u64(seed);
        let identity = IdentityKeyPair::generate(&mut rng);
        let registration_id = generate_registration_id(&mut rng);
        Self {
            address: ProtocolAddress::new(name, 1),
            store: InMemoryProtocolStore::new(identity, registration_id),
            registration_id,
            rng,
        }
    }

    /// Creates a one-time prekey, a signed prekey and, if asked, a Kyber prekey. The private
    /// halves go into this device's store, and the public bundle is returned.
    pub fn publish_bundle(&mut self, kyber: bool) -> PreKeyBundle {
        let identity = self.store.identity_key_pair().unwrap();
        let pre = generate_pre_keys(self.next_id(), 1, &mut self.rng)
            .unwrap()
            .pop()
            .unwrap();
        let signed =
            generate_signed_pre_key(&identity, self.next_id(), 1_700_000_000_000, &mut self.rng)
                .unwrap();
        self.store.store_pre_key(&pre).unwrap();
        self.store.store_signed_pre_key(&signed).unwrap();
        let kyber_pre_key = kyber.then(|| {
            let record =
                generate_kyber_pre_key(&identity, self.next_id(), 1_700_000_000_000, &mut self.rng)
                    .unwrap();
            self.store.store_kyber_pre_key(&record).unwrap();
            KyberPreKeyBundle {
                id: record.id(),
                public_key: record.key_pair().public_key.clone(),
                signature: record.signature().to_vec(),
            }
        });
        PreKeyBundle {
            registration_id: self.registration_id,
            device_id: 1,
            pre_key: Some((pre.id(), pre.key_pair().public_key)),
            signed_pre_key_id: signed.id(),
            signed_pre_key_public: *signed.public_key(),
            signed_pre_key_signature: *signed.signature(),
            identity_key: identity.identity_key(),
            kyber_pre_key,
        }
    }

    fn next_id(&mut self) -> u32 {
        use rand::RngCore;
        (self.rng.next_u32() % 1_000_000) + 1
    }
}

/// Makes `alice` start a session with `bob` from `bob`'s bundle.
pub fn establish(alice: &mut Device, bob: &mut Device, kyber: bool) {
    let bundle = bob.publish_bundle(kyber);
    process_prekey_bundle(&mut alice.store, &bob.address, &bundle, &mut alice.rng).unwrap();
}

/// Passes a message through its wire form, as a network would.
pub fn over_the_wire(message: &CiphertextMessage) -> CiphertextMessage {
    match message {
        CiphertextMessage::Signal(_) => {
            CiphertextMessage::Signal(SignalMessage::deserialize(message.serialized()).unwrap())
        }
        CiphertextMessage::PreKey(_) => CiphertextMessage::PreKey(
            PreKeySignalMessage::deserialize(message.serialized()).unwrap(),
        ),
    }
}

/// Encrypts `text` from `sender` to `receiver` and returns the wire-level message.
pub fn send(sender: &mut Device, receiver: &Device, text: &[u8]) -> CiphertextMessage {
    let message = encrypt(&mut sender.store, &receiver.address, text).unwrap();
    over_the_wire(&message)
}

/// Decrypts a wire-level message at `receiver`, which came from `sender`.
pub fn receive(
    receiver: &mut Device,
    sender: &Device,
    message: &CiphertextMessage,
) -> Result<Vec<u8>> {
    match message {
        CiphertextMessage::Signal(m) => {
            decrypt_signal_message(&mut receiver.store, &sender.address, m, &mut receiver.rng)
        }
        CiphertextMessage::PreKey(m) => {
            decrypt_prekey_message(&mut receiver.store, &sender.address, m, &mut receiver.rng)
        }
    }
}

/// Whether an error is the given variant, ignoring its payload.
pub fn is_duplicate(result: &Result<Vec<u8>>) -> bool {
    matches!(result, Err(Error::DuplicatedMessage(..)))
}
