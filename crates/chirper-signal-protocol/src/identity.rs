//! Identity keys: the long-term Curve25519 identity of a device.
//!
//! Port of libsignal `keys/identity/*.go` and `keys/identity/IdentityKeyPair` (AGPL-3.0-only,
//! Signal Messenger LLC). The private half is zeroized on drop and never printed.

use std::fmt;

use prost::Message;
use rand::{CryptoRng, RngCore};
use zeroize::Zeroizing;

use crate::curve::{KeyPair, PrivateKey, PublicKey, SIGNATURE_LENGTH};
use crate::error::{Error, Result};
use crate::proto::{self, IdentityKeyPairStructure};

/// A device's public identity key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityKey {
    public_key: PublicKey,
}

impl IdentityKey {
    /// Wraps a public key as an identity key.
    pub fn new(public_key: PublicKey) -> Self {
        Self { public_key }
    }

    /// Parses the 33-byte serialized form.
    pub fn deserialize(bytes: &[u8]) -> Result<Self> {
        Ok(Self::new(PublicKey::deserialize(bytes)?))
    }

    /// The underlying public key.
    pub fn public_key(&self) -> &PublicKey {
        &self.public_key
    }

    /// The 33-byte serialized form, as carried on the wire.
    pub fn serialize(&self) -> Vec<u8> {
        self.public_key.serialize()
    }
}

/// A device's identity key pair. Zeroized on drop; `Debug` is redacted.
#[derive(Clone)]
pub struct IdentityKeyPair {
    key_pair: KeyPair,
}

impl IdentityKeyPair {
    /// Generates a fresh identity key pair.
    pub fn generate<R: CryptoRng + RngCore>(csprng: &mut R) -> Self {
        Self {
            key_pair: KeyPair::generate(csprng),
        }
    }

    /// Builds a pair from its halves. The halves are not checked to match.
    pub fn new(public_key: IdentityKey, private_key: PrivateKey) -> Self {
        Self {
            key_pair: KeyPair::new(public_key.public_key, private_key),
        }
    }

    /// The public identity key.
    pub fn identity_key(&self) -> IdentityKey {
        IdentityKey::new(self.key_pair.public_key)
    }

    /// The private identity key.
    pub fn private_key(&self) -> &PrivateKey {
        &self.key_pair.private_key
    }

    /// The key pair as a [`KeyPair`], for key agreement.
    pub fn key_pair(&self) -> &KeyPair {
        &self.key_pair
    }

    /// Signs `message` with the identity private key (XEdDSA).
    pub fn sign<R: CryptoRng + RngCore>(
        &self,
        message: &[u8],
        csprng: &mut R,
    ) -> Result<[u8; SIGNATURE_LENGTH]> {
        let signature = self.key_pair.calculate_signature(message, csprng);
        <[u8; SIGNATURE_LENGTH]>::try_from(&*signature)
            .map_err(|_| Error::InvalidArgument("signature length"))
    }

    /// Serializes as `IdentityKeyPairStructure`. The result holds the private key.
    pub fn serialize(&self) -> Zeroizing<Vec<u8>> {
        let structure = IdentityKeyPairStructure {
            public_key: Some(self.key_pair.public_key.serialize()),
            private_key: Some(self.key_pair.private_key.serialize().to_vec()),
        };
        Zeroizing::new(structure.encode_to_vec())
    }

    /// Parses the output of [`IdentityKeyPair::serialize`].
    pub fn deserialize(bytes: &[u8]) -> Result<Self> {
        let structure: IdentityKeyPairStructure = proto::decode(bytes)?;
        let public = proto::required(structure.public_key)?;
        let private = Zeroizing::new(proto::required(structure.private_key)?);
        Ok(Self {
            key_pair: KeyPair::from_public_and_private(&public, &private)?,
        })
    }
}

impl fmt::Debug for IdentityKeyPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("IdentityKeyPair [redacted]")
    }
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    use super::*;

    #[test]
    fn identity_key_round_trip() {
        let mut rng = StdRng::seed_from_u64(1);
        let pair = IdentityKeyPair::generate(&mut rng);
        let key = pair.identity_key();
        let bytes = key.serialize();
        assert_eq!(bytes.len(), 33);
        assert_eq!(IdentityKey::deserialize(&bytes).unwrap(), key);
        assert!(IdentityKey::deserialize(&bytes[..10]).is_err());
    }

    #[test]
    fn identity_key_pair_round_trip_and_signature() {
        let mut rng = StdRng::seed_from_u64(2);
        let pair = IdentityKeyPair::generate(&mut rng);
        let restored = IdentityKeyPair::deserialize(&pair.serialize()).unwrap();
        assert_eq!(restored.identity_key(), pair.identity_key());
        let signature = restored.sign(b"hello", &mut rng).unwrap();
        assert!(
            pair.identity_key()
                .public_key()
                .verify_signature(b"hello", &signature)
        );
    }

    #[test]
    fn identity_key_pair_rejects_garbage() {
        assert!(IdentityKeyPair::deserialize(&[0xff, 0x00, 0x01]).is_err());
        assert!(IdentityKeyPair::deserialize(&[]).is_err());
    }

    #[test]
    fn debug_is_redacted() {
        let pair = IdentityKeyPair::generate(&mut StdRng::seed_from_u64(3));
        assert!(format!("{pair:?}").contains("redacted"));
    }
}
