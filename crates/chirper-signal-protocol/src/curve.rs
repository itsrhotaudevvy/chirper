//! Curve25519 keys, X25519 agreement and XEdDSA signatures.
//!
//! Port of libsignal `rust/core/src/curve.rs` and `rust/core/src/curve/curve25519.rs`
//! (AGPL-3.0-only, Signal Messenger LLC). Only the DJB (Curve25519) key type exists.
//! Serialized public keys are 33 bytes: the type byte `0x05` followed by the 32-byte
//! Montgomery u-coordinate.

use std::fmt;

use curve25519_dalek::constants::ED25519_BASEPOINT_TABLE;
use curve25519_dalek::edwards::EdwardsPoint;
use curve25519_dalek::montgomery::MontgomeryPoint;
use curve25519_dalek::scalar::{Scalar, clamp_integer};
use rand::{CryptoRng, RngCore};
use sha2::{Digest, Sha512};
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, Zeroizing};

use crate::error::{Error, KeyType, Result};

/// Length of an X25519 shared secret.
pub const AGREEMENT_LENGTH: usize = 32;
/// Length of a raw private key.
pub const PRIVATE_KEY_LENGTH: usize = 32;
/// Length of a raw public key (the u-coordinate, without the type byte).
pub const PUBLIC_KEY_LENGTH: usize = 32;
/// Length of an XEdDSA signature.
pub const SIGNATURE_LENGTH: usize = 64;

/// Type byte for DJB keys; `KeyType::Djb.value()` in libsignal `rust/core/src/curve.rs`.
const DJB_TYPE_BYTE: u8 = 0x05;

/// A public key on Curve25519.
#[derive(Clone, Copy, Eq)]
pub struct PublicKey {
    key: [u8; PUBLIC_KEY_LENGTH],
}

impl PublicKey {
    /// Parses a serialized public key: `0x05 || u-coordinate`.
    ///
    /// As in libsignal, trailing bytes after the 32-byte key are accepted and ignored.
    pub fn deserialize(value: &[u8]) -> Result<Self> {
        let (key_type, value) = value.split_first().ok_or(Error::NoKeyTypeIdentifier)?;
        match *key_type {
            DJB_TYPE_BYTE => {
                let key: &[u8; PUBLIC_KEY_LENGTH] = value
                    .first_chunk()
                    .ok_or(Error::BadKeyLength(KeyType::Djb, value.len() + 1))?;
                Ok(Self { key: *key })
            }
            t => Err(Error::BadKeyType(t)),
        }
    }

    /// Builds a key from the raw 32-byte u-coordinate.
    pub fn from_djb_public_key_bytes(bytes: &[u8]) -> Result<Self> {
        let key = <[u8; PUBLIC_KEY_LENGTH]>::try_from(bytes)
            .map_err(|_| Error::BadKeyLength(KeyType::Djb, bytes.len()))?;
        Ok(Self { key })
    }

    /// The raw 32-byte u-coordinate.
    pub fn public_key_bytes(&self) -> &[u8] {
        &self.key
    }

    /// The 33-byte wire serialization: `0x05 || u-coordinate`.
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + PUBLIC_KEY_LENGTH);
        out.push(DJB_TYPE_BYTE);
        out.extend_from_slice(&self.key);
        out
    }

    /// The key type. Always [`KeyType::Djb`] today.
    pub fn key_type(&self) -> KeyType {
        KeyType::Djb
    }

    /// Verifies an XEdDSA signature over a single message.
    pub fn verify_signature(&self, message: &[u8], signature: &[u8]) -> bool {
        self.verify_signature_for_multipart_message(&[message], signature)
    }

    /// Verifies an XEdDSA signature over the concatenation of `message`.
    pub fn verify_signature_for_multipart_message(
        &self,
        message: &[&[u8]],
        signature: &[u8],
    ) -> bool {
        let Ok(signature) = <&[u8; SIGNATURE_LENGTH]>::try_from(signature) else {
            return false;
        };
        verify_xeddsa(&self.key, message, signature)
    }

    /// Whether the point lies in the prime-order subgroup and the scalar is in range.
    pub fn is_canonical(&self) -> bool {
        is_torsion_free(&self.key) && scalar_is_in_range(&self.key)
    }
}

impl PartialEq for PublicKey {
    fn eq(&self, other: &Self) -> bool {
        bool::from(self.key[..].ct_eq(&other.key[..]))
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Public keys are not secret, but keep Debug output short and free of key material.
        f.write_str("PublicKey { key_type=Djb }")
    }
}

/// A Curve25519 private key. Zeroized on drop; `Debug` is redacted.
#[derive(Clone)]
pub struct PrivateKey {
    key: Zeroizing<[u8; PRIVATE_KEY_LENGTH]>,
}

impl PrivateKey {
    /// Parses a 32-byte private key, clamping it as libsignal does.
    pub fn deserialize(value: &[u8]) -> Result<Self> {
        let key = <[u8; PRIVATE_KEY_LENGTH]>::try_from(value)
            .map_err(|_| Error::BadKeyLength(KeyType::Djb, value.len()))?;
        Ok(Self::from_bytes(key))
    }

    fn from_bytes(bytes: [u8; PRIVATE_KEY_LENGTH]) -> Self {
        // Clamping is not necessary for security but is kept for compatibility with libsignal.
        let mut clamped = clamp_integer(bytes);
        let key = Zeroizing::new(clamped);
        clamped.zeroize();
        Self { key }
    }

    /// The 32 raw key bytes. Callers must treat the result as secret.
    pub fn serialize(&self) -> Zeroizing<Vec<u8>> {
        Zeroizing::new(self.key.to_vec())
    }

    /// Derives the matching public key.
    pub fn public_key(&self) -> PublicKey {
        let key = MontgomeryPoint::mul_base_clamped(*self.key).to_bytes();
        PublicKey { key }
    }

    /// The key type. Always [`KeyType::Djb`] today.
    pub fn key_type(&self) -> KeyType {
        KeyType::Djb
    }

    /// Calculates an XEdDSA signature over a single message.
    pub fn calculate_signature<R: CryptoRng + RngCore>(
        &self,
        message: &[u8],
        csprng: &mut R,
    ) -> Box<[u8]> {
        self.calculate_signature_for_multipart_message(&[message], csprng)
    }

    /// Calculates an XEdDSA signature over the concatenation of `message`.
    pub fn calculate_signature_for_multipart_message<R: CryptoRng + RngCore>(
        &self,
        message: &[&[u8]],
        csprng: &mut R,
    ) -> Box<[u8]> {
        let mut random_bytes = [0u8; 64];
        csprng.fill_bytes(&mut random_bytes);
        let sig = sign_xeddsa(&self.key, message, &random_bytes);
        random_bytes.zeroize();
        Box::new(sig)
    }

    /// X25519 Diffie-Hellman with `their_key`. Rejects the all-zero output in constant time.
    pub fn calculate_agreement(&self, their_key: &PublicKey) -> Result<Zeroizing<[u8; 32]>> {
        let shared = Zeroizing::new(
            MontgomeryPoint(their_key.key)
                .mul_clamped(*self.key)
                .to_bytes(),
        );
        if bool::from(shared.ct_eq(&[0u8; AGREEMENT_LENGTH])) {
            return Err(Error::InvalidKeyAgreement);
        }
        Ok(shared)
    }
}

impl fmt::Debug for PrivateKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PrivateKey [redacted]")
    }
}

impl PartialEq for PrivateKey {
    fn eq(&self, other: &Self) -> bool {
        bool::from(self.key[..].ct_eq(&other.key[..]))
    }
}

impl Eq for PrivateKey {}

/// A public/private key pair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyPair {
    /// The public half.
    pub public_key: PublicKey,
    /// The private half.
    pub private_key: PrivateKey,
}

impl KeyPair {
    /// Generates a fresh key pair from `csprng`.
    pub fn generate<R: CryptoRng + RngCore>(csprng: &mut R) -> Self {
        let mut bytes = Zeroizing::new([0u8; PRIVATE_KEY_LENGTH]);
        csprng.fill_bytes(&mut *bytes);
        let private_key = PrivateKey::from_bytes(*bytes);
        let public_key = private_key.public_key();
        Self {
            public_key,
            private_key,
        }
    }

    /// Builds a pair from parts without checking that they match.
    pub fn new(public_key: PublicKey, private_key: PrivateKey) -> Self {
        Self {
            public_key,
            private_key,
        }
    }

    /// Parses a pair from serialized parts.
    pub fn from_public_and_private(public_key: &[u8], private_key: &[u8]) -> Result<Self> {
        Ok(Self {
            public_key: PublicKey::deserialize(public_key)?,
            private_key: PrivateKey::deserialize(private_key)?,
        })
    }

    /// Signs `message` with the private half.
    pub fn calculate_signature<R: CryptoRng + RngCore>(
        &self,
        message: &[u8],
        csprng: &mut R,
    ) -> Box<[u8]> {
        self.private_key.calculate_signature(message, csprng)
    }

    /// X25519 agreement with `their_key`.
    pub fn calculate_agreement(&self, their_key: &PublicKey) -> Result<Zeroizing<[u8; 32]>> {
        self.private_key.calculate_agreement(their_key)
    }
}

impl TryFrom<PrivateKey> for KeyPair {
    type Error = Error;

    fn try_from(private_key: PrivateKey) -> Result<Self> {
        let public_key = private_key.public_key();
        Ok(Self {
            public_key,
            private_key,
        })
    }
}

// --- Internal curve arithmetic (libsignal rust/core/src/curve/curve25519.rs) ---

/// Hash prefix for the signing nonce: 0xFE followed by 31 bytes of 0xFF (XEdDSA, section 2.3).
const XEDDSA_HASH_PREFIX: [u8; 32] = {
    let mut p = [0xFFu8; 32];
    p[0] = 0xFE;
    p
};

fn sign_xeddsa(
    key_data: &[u8; PRIVATE_KEY_LENGTH],
    message: &[&[u8]],
    random_bytes: &[u8; 64],
) -> [u8; SIGNATURE_LENGTH] {
    let a = Scalar::from_bytes_mod_order(*key_data);
    let ed_public_key = (&a * ED25519_BASEPOINT_TABLE).compress();
    let sign_bit = ed_public_key.as_bytes()[31] & 0b1000_0000_u8;

    let mut hash1 = Sha512::new();
    hash1.update(XEDDSA_HASH_PREFIX);
    hash1.update(key_data);
    for piece in message {
        hash1.update(piece);
    }
    hash1.update(random_bytes);
    let r = Scalar::from_hash(hash1);
    let cap_r = (&r * ED25519_BASEPOINT_TABLE).compress();

    let mut hash = Sha512::new();
    hash.update(cap_r.as_bytes());
    hash.update(ed_public_key.as_bytes());
    for piece in message {
        hash.update(piece);
    }
    let h = Scalar::from_hash(hash);
    let s = (h * a) + r;

    let mut result = [0u8; SIGNATURE_LENGTH];
    result[..32].copy_from_slice(cap_r.as_bytes());
    result[32..].copy_from_slice(s.as_bytes());
    // The sign bit of the Edwards key is carried in the top bit of the signature, as in
    // libsignal-protocol-java compatibility mode.
    result[SIGNATURE_LENGTH - 1] &= 0b0111_1111_u8;
    result[SIGNATURE_LENGTH - 1] |= sign_bit;
    result
}

fn verify_xeddsa(
    their_public_key: &[u8; PUBLIC_KEY_LENGTH],
    message: &[&[u8]],
    signature: &[u8; SIGNATURE_LENGTH],
) -> bool {
    let mont_point = MontgomeryPoint(*their_public_key);
    let Some(ed_pub_key_point) =
        mont_point.to_edwards((signature[SIGNATURE_LENGTH - 1] & 0b1000_0000_u8) >> 7)
    else {
        return false;
    };
    let cap_a = ed_pub_key_point.compress();
    let mut cap_r = [0u8; 32];
    cap_r.copy_from_slice(&signature[..32]);
    let mut s = [0u8; 32];
    s.copy_from_slice(&signature[32..]);
    s[31] &= 0b0111_1111_u8;
    if (s[31] & 0b1110_0000_u8) != 0 {
        return false;
    }
    let minus_cap_a = -ed_pub_key_point;

    let mut hash = Sha512::new();
    hash.update(cap_r);
    hash.update(cap_a.as_bytes());
    for piece in message {
        hash.update(piece);
    }
    let h = Scalar::from_hash(hash);

    let cap_r_check: EdwardsPoint = EdwardsPoint::vartime_double_scalar_mul_basepoint(
        &h,
        &minus_cap_a,
        &Scalar::from_bytes_mod_order(s),
    );
    bool::from(cap_r_check.compress().as_bytes().ct_eq(&cap_r))
}

/// Whether the u-coordinate maps to a point with no small-order component.
fn is_torsion_free(key: &[u8; PUBLIC_KEY_LENGTH]) -> bool {
    MontgomeryPoint(*key)
        .to_edwards(0)
        .is_some_and(|ed| ed.is_torsion_free())
}

/// Whether the top bit and the range above 2^255 - 19 are clear.
fn scalar_is_in_range(k: &[u8; PUBLIC_KEY_LENGTH]) -> bool {
    !(k[31] & 0b1000_0000_u8 != 0
        || (k[0] >= 0u8.wrapping_sub(19) && k[1..31] == [0xFFu8; 30] && k[31] == 0x7F))
}

#[cfg(test)]
mod tests {
    use rand::rngs::OsRng;

    use super::*;

    fn bytes32(s: &str) -> [u8; 32] {
        hex::decode(s).unwrap().try_into().unwrap()
    }

    #[test]
    fn test_agreement_kat() {
        // libsignal rust/core/src/curve/curve25519.rs::test_agreement
        let alice_public =
            bytes32("1bb75966f2e93a3691dfff942bb2a466a1c08b8d78ca3f4d6df8b8bfa2e4ee28");
        let alice_private =
            bytes32("c806439dc9d2c476ffed8f2580c0888d58ab406bf7ae3698879021b96bb4bf59");
        let bob_public =
            bytes32("653614993d2b15ee9e5fd3d86ce719ef4ec1daae1886a87b3f5fa9565a27a22f");
        let bob_private =
            bytes32("b03b34c33a1c44f225b662d2bf4859b8135411fa7b0386d45fb75dc5b91b4466");
        let shared = bytes32("325f239328941ced6e673b86ba41017448e99b649a9c3806c1dd7ca4c477e629");

        let alice = PrivateKey::deserialize(&alice_private).unwrap();
        let bob = PrivateKey::deserialize(&bob_private).unwrap();
        assert_eq!(alice.public_key().public_key_bytes(), alice_public);
        assert_eq!(bob.public_key().public_key_bytes(), bob_public);
        assert_eq!(
            *alice
                .calculate_agreement(&PublicKey::from_djb_public_key_bytes(&bob_public).unwrap())
                .unwrap(),
            shared
        );
        assert_eq!(
            *bob.calculate_agreement(&PublicKey::from_djb_public_key_bytes(&alice_public).unwrap())
                .unwrap(),
            shared
        );
    }

    #[test]
    fn test_random_agreements() {
        let mut csprng = OsRng;
        for _ in 0..50 {
            let alice = KeyPair::generate(&mut csprng);
            let bob = KeyPair::generate(&mut csprng);
            let a = alice.calculate_agreement(&bob.public_key).unwrap();
            let b = bob.calculate_agreement(&alice.public_key).unwrap();
            assert_eq!(*a, *b);
        }
    }

    #[test]
    fn test_signature_kat() {
        // libsignal rust/core/src/curve/curve25519.rs::test_signature
        let identity_private = PrivateKey::deserialize(&bytes32(
            "c097248412e58bf05df487968205132794178e367637f5818f81e0e6ce73e865",
        ))
        .unwrap();
        let identity_public =
            bytes32("ab7e717d4a163b7d9a1d8071dfe9dcf8cdcd1cea3339b6356be84d887e322c64");
        let ephemeral_public =
            hex::decode("05edce9d9c415ca78cb7252e72c2c4a554d3eb29485a0e1d503118d1a82d99fb4a")
                .unwrap();
        let signature = hex::decode(
            "5de88ca9a89b4a115da79109c67c9c7464a3e4180274f1cb8c63c2984e286dfbede82deb9dcd9fae0bfbb821569b3d9001bd8130cd11d486cef047bd60b86e88",
        )
        .unwrap();

        assert_eq!(
            identity_private.public_key().public_key_bytes(),
            identity_public
        );
        let public = PublicKey::from_djb_public_key_bytes(&identity_public).unwrap();
        assert!(public.verify_signature_for_multipart_message(&[&ephemeral_public], &signature));

        for i in 0..signature.len() {
            let mut bad = signature.clone();
            bad[i] ^= 0x01;
            assert!(
                !public.verify_signature_for_multipart_message(&[&ephemeral_public], &bad),
                "signature check passed when it should not have (byte {i})"
            );
        }
    }

    #[test]
    fn test_random_signatures() {
        let mut csprng = OsRng;
        for _ in 0..50 {
            let mut message = [0u8; 64];
            csprng.fill_bytes(&mut message);
            let key = KeyPair::generate(&mut csprng);
            let signature = key.calculate_signature(&message, &mut csprng);
            assert!(key.public_key.verify_signature(&message, &signature));
        }
    }

    #[test]
    fn test_decode_size() {
        let mut csprng = OsRng;
        let key = KeyPair::generate(&mut csprng);
        let serialized = key.public_key.serialize();
        assert_eq!(serialized.len(), 33);
        assert_eq!(serialized, key.private_key.public_key().serialize());
        assert_eq!(PublicKey::deserialize(&serialized).unwrap(), key.public_key);
        assert!(PublicKey::deserialize(&serialized[1..]).is_err());
        assert!(PublicKey::deserialize(&[]).is_err());
        let mut bad_type = serialized.clone();
        bad_type[0] = 0x01;
        assert!(PublicKey::deserialize(&bad_type).is_err());
        let mut extra = serialized.clone();
        extra.push(0);
        assert_eq!(PublicKey::deserialize(&extra).unwrap(), key.public_key);
    }

    #[test]
    fn test_torsion_and_range() {
        assert!(scalar_is_in_range(&[0; 32]));
        let mut high = [0u8; 32];
        high[31] = 0x80;
        assert!(!scalar_is_in_range(&high));
        assert!(!scalar_is_in_range(&[0xFF; 32]));
        // 2^255 - 20 is the largest in-range value.
        let mut p = [0xFFu8; 32];
        p[0] = 0xEC;
        p[31] = 0x7F;
        assert!(scalar_is_in_range(&p));
        let key = KeyPair::generate(&mut OsRng);
        assert!(key.public_key.is_canonical());
    }

    #[test]
    fn test_debug_is_redacted() {
        let key = KeyPair::generate(&mut OsRng);
        let shown = format!("{:?}", key.private_key);
        assert!(shown.contains("redacted"));
        assert!(!shown.contains(&hex::encode(key.private_key.serialize().as_slice())));
    }
}
