//! Prekeys, Kyber prekeys and the prekey bundle a sender fetches to start a session.
//!
//! Port of libsignal `keys/prekey/PreKeyBundle.go`, `state/record/{PreKeyRecord,SignedPreKeyRecord}.go`,
//! `util/keyhelper/KeyHelper.go`, and the Kyber prekey record of the Rust implementation
//! (AGPL-3.0-only, Signal Messenger LLC). Records that hold private keys serialize into a
//! zeroizing buffer and never print them.

use prost::Message;
use rand::{CryptoRng, RngCore};
use zeroize::Zeroizing;

use crate::curve::{KeyPair, PublicKey, SIGNATURE_LENGTH};
use crate::error::{Error, Result};
use crate::identity::{IdentityKey, IdentityKeyPair};
use crate::kem;
use crate::proto::{
    self, KyberPreKeyRecordStructure, PreKeyRecordStructure, SignedPreKeyRecordStructure,
};

/// Largest registration id: libsignal uses 14-bit registration ids.
const REGISTRATION_ID_MASK: u32 = 0x3FFF;

/// Draws a non-zero 14-bit registration id.
pub fn generate_registration_id<R: CryptoRng + RngCore>(csprng: &mut R) -> u32 {
    (csprng.next_u32() & REGISTRATION_ID_MASK).max(1)
}

/// A one-time prekey: a Curve25519 key pair with a numeric id. Private key is zeroized.
#[derive(Clone, Debug)]
pub struct PreKeyRecord {
    id: u32,
    key_pair: KeyPair,
}

impl PreKeyRecord {
    /// Wraps a key pair under an id.
    pub fn new(id: u32, key_pair: KeyPair) -> Self {
        Self { id, key_pair }
    }

    /// The prekey id.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// The key pair.
    pub fn key_pair(&self) -> &KeyPair {
        &self.key_pair
    }

    /// Serializes as `PreKeyRecordStructure`. The result holds the private key.
    pub fn serialize(&self) -> Zeroizing<Vec<u8>> {
        let structure = PreKeyRecordStructure {
            id: Some(self.id),
            public_key: Some(self.key_pair.public_key.serialize()),
            private_key: Some(self.key_pair.private_key.serialize().to_vec()),
        };
        Zeroizing::new(structure.encode_to_vec())
    }

    /// Parses the output of [`PreKeyRecord::serialize`].
    pub fn deserialize(bytes: &[u8]) -> Result<Self> {
        let s: PreKeyRecordStructure = proto::decode(bytes)?;
        let private = Zeroizing::new(proto::required(s.private_key)?);
        let key_pair = KeyPair::from_public_and_private(&proto::required(s.public_key)?, &private)?;
        Ok(Self::new(proto::required(s.id)?, key_pair))
    }
}

/// A signed prekey: a key pair whose public half is signed by the identity key.
#[derive(Clone, Debug)]
pub struct SignedPreKeyRecord {
    id: u32,
    timestamp: u64,
    key_pair: KeyPair,
    signature: [u8; SIGNATURE_LENGTH],
}

impl SignedPreKeyRecord {
    /// Wraps a key pair, its signature and creation time (milliseconds since the Unix epoch).
    pub fn new(
        id: u32,
        timestamp: u64,
        key_pair: KeyPair,
        signature: [u8; SIGNATURE_LENGTH],
    ) -> Self {
        Self {
            id,
            timestamp,
            key_pair,
            signature,
        }
    }

    /// The prekey id.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Creation time in milliseconds since the Unix epoch.
    pub fn timestamp(&self) -> u64 {
        self.timestamp
    }

    /// The key pair.
    pub fn key_pair(&self) -> &KeyPair {
        &self.key_pair
    }

    /// The public key.
    pub fn public_key(&self) -> &PublicKey {
        &self.key_pair.public_key
    }

    /// The XEdDSA signature over the serialized public key.
    pub fn signature(&self) -> &[u8; SIGNATURE_LENGTH] {
        &self.signature
    }

    /// Serializes as `SignedPreKeyRecordStructure`. The result holds the private key.
    pub fn serialize(&self) -> Zeroizing<Vec<u8>> {
        let structure = SignedPreKeyRecordStructure {
            id: Some(self.id),
            public_key: Some(self.key_pair.public_key.serialize()),
            private_key: Some(self.key_pair.private_key.serialize().to_vec()),
            signature: Some(self.signature.to_vec()),
            timestamp: Some(self.timestamp),
        };
        Zeroizing::new(structure.encode_to_vec())
    }

    /// Parses the output of [`SignedPreKeyRecord::serialize`].
    pub fn deserialize(bytes: &[u8]) -> Result<Self> {
        let s: SignedPreKeyRecordStructure = proto::decode(bytes)?;
        let private = Zeroizing::new(proto::required(s.private_key)?);
        let key_pair = KeyPair::from_public_and_private(&proto::required(s.public_key)?, &private)?;
        let signature = <[u8; SIGNATURE_LENGTH]>::try_from(proto::required(s.signature)?)
            .map_err(|_| Error::InvalidProtobufEncoding)?;
        Ok(Self::new(
            proto::required(s.id)?,
            proto::required(s.timestamp)?,
            key_pair,
            signature,
        ))
    }
}

/// A Kyber1024 prekey, signed by the identity key over its serialized public key.
#[derive(Clone, Debug)]
pub struct KyberPreKeyRecord {
    id: u32,
    timestamp: u64,
    key_pair: kem::KeyPair,
    signature: Vec<u8>,
}

impl KyberPreKeyRecord {
    /// Wraps a KEM key pair, its signature and creation time (milliseconds since the Unix epoch).
    pub fn new(id: u32, timestamp: u64, key_pair: kem::KeyPair, signature: Vec<u8>) -> Self {
        Self {
            id,
            timestamp,
            key_pair,
            signature,
        }
    }

    /// The prekey id.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Creation time in milliseconds since the Unix epoch.
    pub fn timestamp(&self) -> u64 {
        self.timestamp
    }

    /// The KEM key pair.
    pub fn key_pair(&self) -> &kem::KeyPair {
        &self.key_pair
    }

    /// The XEdDSA signature over the serialized KEM public key.
    pub fn signature(&self) -> &[u8] {
        &self.signature
    }

    /// Serializes as `KyberPreKeyRecordStructure`. The result holds the secret key.
    pub fn serialize(&self) -> Zeroizing<Vec<u8>> {
        let structure = KyberPreKeyRecordStructure {
            id: Some(self.id),
            public_key: Some(self.key_pair.public_key.serialize().into_vec()),
            private_key: Some(self.key_pair.secret_key.serialize().to_vec()),
            signature: Some(self.signature.clone()),
            timestamp: Some(self.timestamp),
        };
        Zeroizing::new(structure.encode_to_vec())
    }

    /// Parses the output of [`KyberPreKeyRecord::serialize`].
    pub fn deserialize(bytes: &[u8]) -> Result<Self> {
        let s: KyberPreKeyRecordStructure = proto::decode(bytes)?;
        let secret = Zeroizing::new(proto::required(s.private_key)?);
        let key_pair =
            kem::KeyPair::from_public_and_private(&proto::required(s.public_key)?, &secret)?;
        Ok(Self::new(
            proto::required(s.id)?,
            proto::required(s.timestamp)?,
            key_pair,
            proto::required(s.signature)?,
        ))
    }
}

/// The Kyber half of a prekey bundle: id, public key and signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KyberPreKeyBundle {
    /// Kyber prekey id.
    pub id: u32,
    /// Kyber public key.
    pub public_key: kem::PublicKey,
    /// Identity signature over the serialized public key.
    pub signature: Vec<u8>,
}

/// Everything a sender needs from a recipient device to start a session (`PreKeyBundle`).
#[derive(Clone, Debug)]
pub struct PreKeyBundle {
    /// The recipient's registration id.
    pub registration_id: u32,
    /// The recipient's device id.
    pub device_id: u32,
    /// Optional one-time prekey: its id and public key.
    pub pre_key: Option<(u32, PublicKey)>,
    /// Signed prekey id.
    pub signed_pre_key_id: u32,
    /// Signed prekey public key.
    pub signed_pre_key_public: PublicKey,
    /// Identity signature over the serialized signed prekey.
    pub signed_pre_key_signature: [u8; SIGNATURE_LENGTH],
    /// The recipient's identity key.
    pub identity_key: IdentityKey,
    /// Optional Kyber prekey, enabling PQXDH (session version 4).
    pub kyber_pre_key: Option<KyberPreKeyBundle>,
}

/// Generates one-time prekeys with ids `start_id .. start_id + count`.
pub fn generate_pre_keys<R: CryptoRng + RngCore>(
    start_id: u32,
    count: u32,
    csprng: &mut R,
) -> Result<Vec<PreKeyRecord>> {
    (0..count)
        .map(|offset| {
            let id = start_id.checked_add(offset).ok_or(Error::InvalidPreKeyId)?;
            Ok(PreKeyRecord::new(id, KeyPair::generate(csprng)))
        })
        .collect()
}

/// Generates a signed prekey and signs its public key with the identity key.
pub fn generate_signed_pre_key<R: CryptoRng + RngCore>(
    identity: &IdentityKeyPair,
    id: u32,
    timestamp: u64,
    csprng: &mut R,
) -> Result<SignedPreKeyRecord> {
    let key_pair = KeyPair::generate(csprng);
    let signature = identity.sign(&key_pair.public_key.serialize(), csprng)?;
    Ok(SignedPreKeyRecord::new(id, timestamp, key_pair, signature))
}

/// Generates a Kyber1024 prekey and signs its public key with the identity key.
pub fn generate_kyber_pre_key<R: CryptoRng + RngCore>(
    identity: &IdentityKeyPair,
    id: u32,
    timestamp: u64,
    csprng: &mut R,
) -> Result<KyberPreKeyRecord> {
    let key_pair = kem::KeyPair::generate(kem::KeyType::Kyber1024, csprng);
    let signature = identity.sign(&key_pair.public_key.serialize(), csprng)?;
    Ok(KyberPreKeyRecord::new(
        id,
        timestamp,
        key_pair,
        signature.to_vec(),
    ))
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    use super::*;

    #[test]
    fn registration_id_is_14_bit_and_nonzero() {
        let mut rng = StdRng::seed_from_u64(4);
        for _ in 0..200 {
            let id = generate_registration_id(&mut rng);
            assert!((1..=REGISTRATION_ID_MASK).contains(&id));
        }
    }

    #[test]
    fn pre_key_round_trip() {
        let mut rng = StdRng::seed_from_u64(5);
        let record = PreKeyRecord::new(9, KeyPair::generate(&mut rng));
        let restored = PreKeyRecord::deserialize(&record.serialize()).unwrap();
        assert_eq!(restored.id(), 9);
        assert_eq!(restored.key_pair(), record.key_pair());
        assert!(PreKeyRecord::deserialize(&[0x0a]).is_err());
    }

    #[test]
    fn generate_pre_keys_assigns_consecutive_ids() {
        let mut rng = StdRng::seed_from_u64(6);
        let keys = generate_pre_keys(1, 3, &mut rng).unwrap();
        let ids: Vec<u32> = keys.iter().map(PreKeyRecord::id).collect();
        assert_eq!(ids, vec![1, 2, 3]);
        assert!(generate_pre_keys(u32::MAX, 2, &mut rng).is_err());
    }

    #[test]
    fn signed_pre_key_is_signed_by_identity() {
        let mut rng = StdRng::seed_from_u64(7);
        let identity = IdentityKeyPair::generate(&mut rng);
        let record = generate_signed_pre_key(&identity, 3, 1_700_000_000_000, &mut rng).unwrap();
        assert!(
            identity
                .identity_key()
                .public_key()
                .verify_signature(&record.public_key().serialize(), record.signature())
        );
        let restored = SignedPreKeyRecord::deserialize(&record.serialize()).unwrap();
        assert_eq!(restored.id(), 3);
        assert_eq!(restored.timestamp(), 1_700_000_000_000);
        assert_eq!(restored.public_key(), record.public_key());
        assert_eq!(restored.signature(), record.signature());
    }

    #[test]
    fn kyber_pre_key_is_signed_and_round_trips() {
        let mut rng = StdRng::seed_from_u64(8);
        let identity = IdentityKeyPair::generate(&mut rng);
        let record = generate_kyber_pre_key(&identity, 5, 42, &mut rng).unwrap();
        assert!(identity.identity_key().public_key().verify_signature(
            &record.key_pair().public_key.serialize(),
            record.signature()
        ));
        let restored = KyberPreKeyRecord::deserialize(&record.serialize()).unwrap();
        assert_eq!(restored.id(), 5);
        assert_eq!(restored.key_pair().public_key, record.key_pair().public_key);
        assert_eq!(restored.signature(), record.signature());
    }

    #[test]
    fn debug_hides_private_keys() {
        let mut rng = StdRng::seed_from_u64(9);
        let record = PreKeyRecord::new(1, KeyPair::generate(&mut rng));
        assert!(!format!("{record:?}").contains(&hex::encode(
            record.key_pair().private_key.serialize().as_slice()
        )));
    }
}
