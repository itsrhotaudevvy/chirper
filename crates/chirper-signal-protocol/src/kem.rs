//! Key encapsulation mechanisms: Kyber1024 (type byte 0x08) and ML-KEM-1024 (type byte 0x0A).
//!
//! Port of libsignal `rust/protocol/src/kem.rs` and `rust/protocol/src/kem/{kyber1024,mlkem1024}.rs`
//! (AGPL-3.0-only, Signal Messenger LLC). Serialized keys and ciphertexts carry a one-byte
//! KEM type prefix, followed by the raw libcrux encoding. Kyber768 and ML-KEM-768 are not
//! ported: Signal's PQXDH uses Kyber1024 only.

use std::fmt;

use libcrux_ml_kem::SHARED_SECRET_SIZE;
use libcrux_ml_kem::mlkem1024::{MlKem1024Ciphertext, MlKem1024PrivateKey, MlKem1024PublicKey};
use rand::{CryptoRng, RngCore};
use zeroize::Zeroizing;

use crate::error::{Error, Result};

/// Length of the raw ML-KEM-1024 / Kyber1024 public key.
pub const PUBLIC_KEY_LENGTH: usize = 1568;
/// Length of the raw ML-KEM-1024 / Kyber1024 secret key.
pub const SECRET_KEY_LENGTH: usize = 3168;
/// Length of the raw ML-KEM-1024 / Kyber1024 ciphertext.
pub const CIPHERTEXT_LENGTH: usize = 1568;
/// Length of the shared secret.
pub const SHARED_SECRET_LENGTH: usize = SHARED_SECRET_SIZE;

/// Key-generation seed size (`KEY_GENERATION_SEED_SIZE` in libcrux).
const KEY_GENERATION_SEED_LENGTH: usize = 64;
/// Encapsulation seed size (`ENCAPS_SEED_SIZE` in libcrux).
const ENCAPS_SEED_LENGTH: usize = 32;

/// Designates a supported KEM protocol.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum KeyType {
    /// Kyber1024 (the pre-standard Kyber variant used by PQXDH). Type byte 0x08.
    Kyber1024,
    /// ML-KEM-1024 (FIPS 203). Type byte 0x0A.
    MlKem1024,
}

impl fmt::Display for KeyType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl KeyType {
    /// The protocol identifier byte prepended to serialized keys and ciphertexts.
    pub fn value(self) -> u8 {
        match self {
            KeyType::Kyber1024 => 0x08,
            KeyType::MlKem1024 => 0x0A,
        }
    }
}

impl TryFrom<u8> for KeyType {
    type Error = Error;

    fn try_from(x: u8) -> Result<Self> {
        match x {
            0x08 => Ok(KeyType::Kyber1024),
            0x0A => Ok(KeyType::MlKem1024),
            t => Err(Error::BadKemKeyType(t)),
        }
    }
}

/// The shared secret and the serialized ciphertext produced by [`PublicKey::encapsulate`].
pub type Encapsulation = (Zeroizing<Vec<u8>>, Box<[u8]>);

/// A KEM public key.
#[derive(Clone, PartialEq, Eq)]
pub struct PublicKey {
    key_type: KeyType,
    data: Box<[u8]>,
}

/// A KEM secret key. Zeroized on drop; `Debug` is redacted.
#[derive(Clone)]
pub struct SecretKey {
    key_type: KeyType,
    data: Zeroizing<Vec<u8>>,
}

/// A public/secret key pair for a KEM.
#[derive(Clone)]
pub struct KeyPair {
    /// The public half.
    pub public_key: PublicKey,
    /// The secret half.
    pub secret_key: SecretKey,
}

impl PublicKey {
    /// Parses `type byte || raw key` and checks the length for the type.
    pub fn deserialize(value: &[u8]) -> Result<Self> {
        let (&t, data) = value.split_first().ok_or(Error::NoKeyTypeIdentifier)?;
        let key_type = KeyType::try_from(t)?;
        if data.len() != PUBLIC_KEY_LENGTH {
            return Err(Error::BadKemKeyLength(key_type, value.len()));
        }
        Ok(Self {
            key_type,
            data: data.into(),
        })
    }

    /// `type byte || raw key`.
    pub fn serialize(&self) -> Box<[u8]> {
        let mut out = Vec::with_capacity(1 + self.data.len());
        out.push(self.key_type.value());
        out.extend_from_slice(&self.data);
        out.into_boxed_slice()
    }

    /// The KEM this key belongs to.
    pub fn key_type(&self) -> KeyType {
        self.key_type
    }

    /// Encapsulates a fresh shared secret. Returns `(shared_secret, serialized_ciphertext)`.
    pub fn encapsulate<R: CryptoRng + RngCore>(&self, csprng: &mut R) -> Result<Encapsulation> {
        let mut seed = Zeroizing::new([0u8; ENCAPS_SEED_LENGTH]);
        csprng.fill_bytes(&mut *seed);
        let pk = MlKem1024PublicKey::try_from(&self.data[..])
            .map_err(|_| Error::BadKemKeyLength(self.key_type, self.data.len()))?;
        let (ct, ss) = match self.key_type {
            // Kyber1024 and ML-KEM-1024 share the libcrux ML-KEM-1024 encapsulation routine;
            // Kyber differs only in its key-derivation hash, which libcrux selects internally.
            KeyType::Kyber1024 => libcrux_ml_kem::kyber1024::encapsulate(&pk, *seed),
            KeyType::MlKem1024 => libcrux_ml_kem::mlkem1024::encapsulate(&pk, *seed),
        };
        let mut out = Vec::with_capacity(1 + CIPHERTEXT_LENGTH);
        out.push(self.key_type.value());
        out.extend_from_slice(ct.as_ref());
        Ok((Zeroizing::new(ss.as_ref().to_vec()), out.into_boxed_slice()))
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PublicKey")
            .field("key_type", &self.key_type)
            .finish_non_exhaustive()
    }
}

impl SecretKey {
    /// Parses `type byte || raw key` and checks the length for the type.
    pub fn deserialize(value: &[u8]) -> Result<Self> {
        let (&t, data) = value.split_first().ok_or(Error::NoKeyTypeIdentifier)?;
        let key_type = KeyType::try_from(t)?;
        if data.len() != SECRET_KEY_LENGTH {
            return Err(Error::BadKemKeyLength(key_type, value.len()));
        }
        Ok(Self {
            key_type,
            data: Zeroizing::new(data.to_vec()),
        })
    }

    /// `type byte || raw key`. The result is secret.
    pub fn serialize(&self) -> Zeroizing<Vec<u8>> {
        let mut out = Vec::with_capacity(1 + self.data.len());
        out.push(self.key_type.value());
        out.extend_from_slice(&self.data);
        Zeroizing::new(out)
    }

    /// The KEM this key belongs to.
    pub fn key_type(&self) -> KeyType {
        self.key_type
    }

    /// Recovers the shared secret from a serialized ciphertext.
    pub fn decapsulate(&self, ciphertext: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        let (&t, raw) = ciphertext.split_first().ok_or(Error::NoKeyTypeIdentifier)?;
        let ct_type = KeyType::try_from(t)?;
        if ct_type != self.key_type {
            return Err(Error::WrongKemKeyType(
                ct_type.value(),
                self.key_type.value(),
            ));
        }
        if raw.len() != CIPHERTEXT_LENGTH {
            return Err(Error::BadKemCiphertextLength(
                self.key_type,
                ciphertext.len(),
            ));
        }
        let sk = MlKem1024PrivateKey::try_from(&self.data[..])
            .map_err(|_| Error::BadKemKeyLength(self.key_type, self.data.len()))?;
        let ct = MlKem1024Ciphertext::try_from(raw)
            .map_err(|_| Error::BadKemCiphertextLength(self.key_type, ciphertext.len()))?;
        let ss = match self.key_type {
            KeyType::Kyber1024 => libcrux_ml_kem::kyber1024::decapsulate(&sk, &ct),
            KeyType::MlKem1024 => libcrux_ml_kem::mlkem1024::decapsulate(&sk, &ct),
        };
        Ok(Zeroizing::new(ss.as_ref().to_vec()))
    }
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretKey")
            .field("key_type", &self.key_type)
            .field("key", &"[redacted]")
            .finish()
    }
}

impl KeyPair {
    /// Generates a fresh key pair of the given type.
    pub fn generate<R: CryptoRng + RngCore>(key_type: KeyType, csprng: &mut R) -> Self {
        let mut seed = Zeroizing::new([0u8; KEY_GENERATION_SEED_LENGTH]);
        csprng.fill_bytes(&mut *seed);
        let (sk, pk) = match key_type {
            KeyType::Kyber1024 => libcrux_ml_kem::kyber1024::generate_key_pair(*seed),
            KeyType::MlKem1024 => libcrux_ml_kem::mlkem1024::generate_key_pair(*seed),
        }
        .into_parts();
        Self {
            public_key: PublicKey {
                key_type,
                data: pk.as_ref().into(),
            },
            secret_key: SecretKey {
                key_type,
                data: Zeroizing::new(sk.as_ref().to_vec()),
            },
        }
    }

    /// Deserializes both halves; they must have the same KEM type.
    pub fn from_public_and_private(public_key: &[u8], secret_key: &[u8]) -> Result<Self> {
        let public_key = PublicKey::deserialize(public_key)?;
        let secret_key = SecretKey::deserialize(secret_key)?;
        if public_key.key_type != secret_key.key_type {
            return Err(Error::WrongKemKeyType(
                secret_key.key_type.value(),
                public_key.key_type.value(),
            ));
        }
        Ok(Self {
            public_key,
            secret_key,
        })
    }
}

impl fmt::Debug for KeyPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyPair")
            .field("public_key", &self.public_key)
            .field("secret_key", &self.secret_key)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use rand::rngs::OsRng;

    use super::*;

    // Test data copied from libsignal rust/protocol/src/kem/test-data/ (AGPL-3.0-only).
    // pk.dat and sk.dat are raw Kyber1024 keys; the ML-KEM files already carry their 0x0A type byte.
    const KYBER_PK: &[u8] = include_bytes!("../tests/data/pk.dat");
    const KYBER_SK: &[u8] = include_bytes!("../tests/data/sk.dat");
    const MLKEM_PK: &[u8] = include_bytes!("../tests/data/mlkem-pk.dat");
    const MLKEM_SK: &[u8] = include_bytes!("../tests/data/mlkem-sk.dat");

    fn with_type(t: u8, data: &[u8]) -> Vec<u8> {
        let mut v = vec![t];
        v.extend_from_slice(data);
        v
    }

    #[test]
    fn test_serialize_kyber_fixture() {
        // libsignal kem.rs::test_serialize
        let pk = PublicKey::deserialize(&with_type(0x08, KYBER_PK)).unwrap();
        let sk = SecretKey::deserialize(&with_type(0x08, KYBER_SK)).unwrap();
        assert_eq!(pk.key_type(), KeyType::Kyber1024);
        assert_eq!(&*pk.serialize(), &with_type(0x08, KYBER_PK)[..]);
        assert_eq!(&*sk.serialize(), &with_type(0x08, KYBER_SK)[..]);
    }

    #[test]
    fn test_kyber1024_encaps_decaps_fixture() {
        // libsignal kem.rs::test_kyber1024_kem
        let pk = PublicKey::deserialize(&with_type(0x08, KYBER_PK)).unwrap();
        let sk = SecretKey::deserialize(&with_type(0x08, KYBER_SK)).unwrap();
        let (ss_sender, ct) = pk.encapsulate(&mut OsRng).unwrap();
        assert_eq!(ct.len(), CIPHERTEXT_LENGTH + 1);
        let ss_recipient = sk.decapsulate(&ct).unwrap();
        assert_eq!(*ss_sender, *ss_recipient);
        assert_eq!(ss_sender.len(), SHARED_SECRET_LENGTH);
    }

    #[test]
    fn test_mlkem1024_fixture() {
        // libsignal kem.rs::test_mlkem1024_kem
        let pk = PublicKey::deserialize(MLKEM_PK).unwrap();
        let sk = SecretKey::deserialize(MLKEM_SK).unwrap();
        assert_eq!(pk.key_type(), KeyType::MlKem1024);
        let (ss_sender, ct) = pk.encapsulate(&mut OsRng).unwrap();
        assert_eq!(ct[0], 0x0A);
        assert_eq!(*sk.decapsulate(&ct).unwrap(), *ss_sender);
    }

    #[test]
    fn test_keypair_generation_lengths() {
        for t in [KeyType::Kyber1024, KeyType::MlKem1024] {
            let kp = KeyPair::generate(t, &mut OsRng);
            assert_eq!(kp.secret_key.serialize().len(), SECRET_KEY_LENGTH + 1);
            assert_eq!(kp.public_key.serialize().len(), PUBLIC_KEY_LENGTH + 1);
            let (ss_sender, ct) = kp.public_key.encapsulate(&mut OsRng).unwrap();
            assert_eq!(ct.len(), CIPHERTEXT_LENGTH + 1);
            assert_eq!(*kp.secret_key.decapsulate(&ct).unwrap(), *ss_sender);
        }
    }

    #[test]
    fn test_rejects_bad_types_and_lengths() {
        assert!(matches!(
            PublicKey::deserialize(&[]),
            Err(Error::NoKeyTypeIdentifier)
        ));
        assert!(matches!(
            PublicKey::deserialize(&with_type(0x07, KYBER_PK)),
            Err(Error::BadKemKeyType(0x07))
        ));
        assert!(PublicKey::deserialize(&with_type(0x08, &KYBER_PK[1..])).is_err());
        assert!(SecretKey::deserialize(&with_type(0x0A, &KYBER_SK[1..])).is_err());
        assert!(SecretKey::deserialize(&MLKEM_SK[..MLKEM_SK.len() - 1]).is_err());

        let kyber = KeyPair::generate(KeyType::Kyber1024, &mut OsRng);
        let mlkem = KeyPair::generate(KeyType::MlKem1024, &mut OsRng);
        let (_, ct) = kyber.public_key.encapsulate(&mut OsRng).unwrap();
        assert!(matches!(
            mlkem.secret_key.decapsulate(&ct),
            Err(Error::WrongKemKeyType(0x08, 0x0A))
        ));
        assert!(kyber.secret_key.decapsulate(&ct[..ct.len() - 1]).is_err());
    }
}
