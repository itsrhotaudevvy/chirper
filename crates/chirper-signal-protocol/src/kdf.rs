//! HKDF-SHA256, HMAC chain steps, and the root, chain and message key types.
//!
//! Port of libsignal `kdf/HKDF.go`, `keys/root/RootKey.go`, `keys/chain/ChainKey.go`,
//! `keys/message/MessageKey.go` and `keys/session/DerivedSecrets.go` (AGPL-3.0-only, Signal
//! Messenger LLC). Every key buffer is zeroized on drop and never printed.

use std::fmt;

use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::curve::{KeyPair, PublicKey};
use crate::error::{Error, Result};

/// HKDF info for root and chain derivation in a ratchet step.
pub const ROOT_RATCHET_INFO: &[u8] = b"WhisperRatchet";
/// HKDF info for message key derivation.
pub const MESSAGE_KEYS_INFO: &[u8] = b"WhisperMessageKeys";
/// Input to `HMAC(chain_key, 0x01)` that yields the message key seed.
const MESSAGE_KEY_SEED: [u8; 1] = [0x01];
/// Input to `HMAC(chain_key, 0x02)` that yields the next chain key.
const CHAIN_KEY_SEED: [u8; 1] = [0x02];
/// Length of a root key and of a chain key.
pub const KEY_LENGTH: usize = 32;
/// Length of the message key material: 32 cipher key, 32 MAC key, 16 IV.
const MESSAGE_KEY_MATERIAL_LENGTH: usize = 80;

/// HKDF-SHA256 with the given salt (`None` means a zero-filled salt), producing `len` bytes.
pub fn hkdf_sha256(
    input_key_material: &[u8],
    salt: Option<&[u8]>,
    info: &[u8],
    len: usize,
) -> Result<Zeroizing<Vec<u8>>> {
    let hk = Hkdf::<Sha256>::new(salt, input_key_material);
    let mut out = Zeroizing::new(vec![0u8; len]);
    hk.expand(info, &mut out)
        .map_err(|_| Error::InvalidArgument("hkdf output length"))?;
    Ok(out)
}

/// HMAC-SHA256 over the concatenation of `parts`.
pub fn hmac_sha256(key: &[u8], parts: &[&[u8]]) -> Result<Zeroizing<[u8; 32]>> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key).map_err(|_| Error::InvalidArgument("hmac key"))?;
    for part in parts {
        mac.update(part);
    }
    let mut out = Zeroizing::new([0u8; 32]);
    out.copy_from_slice(&mac.finalize().into_bytes());
    Ok(out)
}

/// Splits 64 bytes of derived secret into a root key (first half) and a chain key (second).
fn split_root_and_chain(derived: &[u8]) -> Result<(RootKey, ChainKey)> {
    let root: [u8; KEY_LENGTH] = derived
        .get(..KEY_LENGTH)
        .and_then(|s| s.try_into().ok())
        .ok_or(Error::InvalidArgument("derived secret length"))?;
    let chain: [u8; KEY_LENGTH] = derived
        .get(KEY_LENGTH..2 * KEY_LENGTH)
        .and_then(|s| s.try_into().ok())
        .ok_or(Error::InvalidArgument("derived secret length"))?;
    Ok((RootKey::new(root), ChainKey::new(chain, 0)))
}

/// Derives a fresh root key and chain key (index 0) from `secrets` with the given HKDF `info`
/// and no salt. Used for the initial X3DH/PQXDH secret.
pub fn derive_root_and_chain(secrets: &[u8], info: &[u8]) -> Result<(RootKey, ChainKey)> {
    let derived = hkdf_sha256(secrets, None, info, 2 * KEY_LENGTH)?;
    split_root_and_chain(&derived)
}

/// A root key of the Double Ratchet. Zeroized on drop.
#[derive(Clone)]
pub struct RootKey {
    key: Zeroizing<[u8; KEY_LENGTH]>,
}

impl RootKey {
    /// Wraps 32 raw root key bytes.
    pub fn new(key: [u8; KEY_LENGTH]) -> Self {
        Self {
            key: Zeroizing::new(key),
        }
    }

    /// The raw root key bytes. Callers must treat the result as secret.
    pub fn bytes(&self) -> &[u8; KEY_LENGTH] {
        &self.key
    }

    /// Performs one DH ratchet step: `HKDF(DH(our, their), salt = root, "WhisperRatchet")`.
    /// Returns the next root key and a fresh chain key at index 0.
    pub fn create_chain(
        &self,
        their_ratchet_key: &PublicKey,
        our_ratchet_key: &KeyPair,
    ) -> Result<(RootKey, ChainKey)> {
        let shared = our_ratchet_key
            .private_key
            .calculate_agreement(their_ratchet_key)?;
        let derived = hkdf_sha256(
            &*shared,
            Some(&self.key[..]),
            ROOT_RATCHET_INFO,
            2 * KEY_LENGTH,
        )?;
        split_root_and_chain(&derived)
    }
}

impl fmt::Debug for RootKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RootKey [redacted]")
    }
}

/// A chain key and the index of the next message it will produce. Zeroized on drop.
#[derive(Clone)]
pub struct ChainKey {
    key: Zeroizing<[u8; KEY_LENGTH]>,
    index: u32,
}

impl ChainKey {
    /// Wraps a chain key at the given index.
    pub fn new(key: [u8; KEY_LENGTH], index: u32) -> Self {
        Self {
            key: Zeroizing::new(key),
            index,
        }
    }

    /// The raw chain key bytes. Callers must treat the result as secret.
    pub fn bytes(&self) -> &[u8; KEY_LENGTH] {
        &self.key
    }

    /// The index of the message whose keys [`ChainKey::message_keys`] returns.
    pub fn index(&self) -> u32 {
        self.index
    }

    /// The chain key for the next index: `HMAC(key, 0x02)`.
    pub fn next(&self) -> Result<ChainKey> {
        let next = hmac_sha256(&self.key[..], &[&CHAIN_KEY_SEED])?;
        let index = self
            .index
            .checked_add(1)
            .ok_or(Error::InvalidArgument("chain index overflow"))?;
        Ok(ChainKey::new(*next, index))
    }

    /// The message keys for this index: `HKDF(HMAC(key, 0x01), "WhisperMessageKeys")`.
    pub fn message_keys(&self) -> Result<MessageKeys> {
        let seed = hmac_sha256(&self.key[..], &[&MESSAGE_KEY_SEED])?;
        let material = hkdf_sha256(
            &seed[..],
            None,
            MESSAGE_KEYS_INFO,
            MESSAGE_KEY_MATERIAL_LENGTH,
        )?;
        MessageKeys::from_material(&material, self.index)
    }
}

impl fmt::Debug for ChainKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChainKey")
            .field("index", &self.index)
            .field("key", &"[redacted]")
            .finish()
    }
}

/// The AES-256 key, HMAC key and IV for one message. Zeroized on drop.
#[derive(Clone)]
pub struct MessageKeys {
    cipher_key: Zeroizing<[u8; 32]>,
    mac_key: Zeroizing<[u8; 32]>,
    iv: [u8; 16],
    index: u32,
}

impl MessageKeys {
    /// Builds message keys from their parts.
    pub fn new(cipher_key: [u8; 32], mac_key: [u8; 32], iv: [u8; 16], index: u32) -> Self {
        Self {
            cipher_key: Zeroizing::new(cipher_key),
            mac_key: Zeroizing::new(mac_key),
            iv,
            index,
        }
    }

    fn from_material(material: &[u8], index: u32) -> Result<Self> {
        let cipher_key: [u8; 32] = material
            .get(..32)
            .and_then(|s| s.try_into().ok())
            .ok_or(Error::InvalidArgument("message key material"))?;
        let mac_key: [u8; 32] = material
            .get(32..64)
            .and_then(|s| s.try_into().ok())
            .ok_or(Error::InvalidArgument("message key material"))?;
        let iv: [u8; 16] = material
            .get(64..80)
            .and_then(|s| s.try_into().ok())
            .ok_or(Error::InvalidArgument("message key material"))?;
        Ok(Self::new(cipher_key, mac_key, iv, index))
    }

    /// The AES-256 key.
    pub fn cipher_key(&self) -> &[u8; 32] {
        &self.cipher_key
    }

    /// The HMAC-SHA256 key.
    pub fn mac_key(&self) -> &[u8; 32] {
        &self.mac_key
    }

    /// The CBC initialisation vector.
    pub fn iv(&self) -> &[u8; 16] {
        &self.iv
    }

    /// The message counter these keys belong to.
    pub fn index(&self) -> u32 {
        self.index
    }
}

impl fmt::Debug for MessageKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MessageKeys")
            .field("index", &self.index)
            .field("keys", &"[redacted]")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hkdf_rfc5869_case1() {
        // RFC 5869 test case 1 (SHA-256).
        let ikm = [0x0bu8; 22];
        let salt = hex::decode("000102030405060708090a0b0c").unwrap();
        let info = hex::decode("f0f1f2f3f4f5f6f7f8f9").unwrap();
        let okm = hkdf_sha256(&ikm, Some(&salt), &info, 42).unwrap();
        assert_eq!(
            hex::encode(&*okm),
            "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
        );
    }

    #[test]
    fn hmac_rfc4231_case2() {
        // RFC 4231 test case 2.
        let mac = hmac_sha256(b"Jefe", &[b"what do ya want ", b"for nothing?"]).unwrap();
        assert_eq!(
            hex::encode(*mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn chain_steps_match_hmac_seeds() {
        // The message seed is HMAC(key, 0x01) and the next key is HMAC(key, 0x02).
        let key = [7u8; 32];
        let chain = ChainKey::new(key, 5);
        let seed = hmac_sha256(&key, &[&[0x01]]).unwrap();
        let next = hmac_sha256(&key, &[&[0x02]]).unwrap();
        let material = hkdf_sha256(&seed[..], None, MESSAGE_KEYS_INFO, 80).unwrap();
        let keys = chain.message_keys().unwrap();
        assert_eq!(keys.cipher_key()[..], material[..32]);
        assert_eq!(keys.mac_key()[..], material[32..64]);
        assert_eq!(keys.iv()[..], material[64..80]);
        assert_eq!(keys.index(), 5);
        let advanced = chain.next().unwrap();
        assert_eq!(*advanced.bytes(), *next);
        assert_eq!(advanced.index(), 6);
    }

    #[test]
    fn known_answer_message_keys() {
        // Computed independently with Python's hmac/hashlib, following libsignal
        // keys/chain/ChainKey.go and keys/message/MessageKey.go:
        // seed = HMAC(0x07*32, 0x01); material = HKDF(seed, salt=zeros, "WhisperMessageKeys", 80).
        let chain = ChainKey::new([7u8; 32], 0);
        let keys = chain.message_keys().unwrap();
        assert_eq!(
            hex::encode(keys.cipher_key()),
            "415a1c77fbf8276cd3bf41a429433e752808a319b74f435dc6bc3559d431a3f5"
        );
        assert_eq!(
            hex::encode(keys.mac_key()),
            "2badf7b2797c63f996633fa2dd60a34da0dc6221ddfed529a12a83eec399af57"
        );
        assert_eq!(hex::encode(keys.iv()), "8004f453ad5f4ddb085fa7da1d9539cc");
        assert_eq!(
            hex::encode(chain.next().unwrap().bytes()),
            "469e03123f1c9c6d5c3ffaa260c7fef9f863c0b6af93f7dcae5c13cbfdadfca5"
        );
    }

    #[test]
    fn chain_key_derivation_kat_from_libsignal() {
        // Ported from libsignal rust/protocol/src/ratchet/keys.rs (`test_chain_key_derivation`),
        // AGPL-3.0-only, Signal Messenger LLC.
        let seed: [u8; 32] =
            hex::decode("8ab72d6f4cc5ac0d387eaf463378ddb28edd07385b1cb01250c715982e7ad48f")
                .unwrap()
                .try_into()
                .unwrap();
        let chain = ChainKey::new(seed, 0);
        let keys = chain.message_keys().unwrap();
        assert_eq!(
            hex::encode(keys.cipher_key()),
            "bf51e9d75e0e31031051f82a2491ffc084fa298b7793bd9db620056febf45217"
        );
        assert_eq!(
            hex::encode(keys.mac_key()),
            "c6c77d6a73a354337a56435e34607dfe48e3ace14e77314dc6abc172e7a7030b"
        );
        assert_eq!(
            hex::encode(chain.next().unwrap().bytes()),
            "28e8f8fee54b801eef7c5cfb2f17f32c7b334485bbb70fac6ec10342a246d15d"
        );
        assert_eq!(chain.next().unwrap().index(), 1);
    }

    #[test]
    fn root_chain_split_is_root_then_chain() {
        let derived: Vec<u8> = (0u8..64).collect();
        let (root, chain) = split_root_and_chain(&derived).unwrap();
        assert_eq!(root.bytes()[..], derived[..32]);
        assert_eq!(chain.bytes()[..], derived[32..]);
        assert_eq!(chain.index(), 0);
        assert!(split_root_and_chain(&derived[..63]).is_err());
    }

    #[test]
    fn debug_is_redacted() {
        let root = RootKey::new([1; 32]);
        assert!(format!("{root:?}").contains("redacted"));
        assert!(!format!("{root:?}").contains("01"));
        let chain = ChainKey::new([1; 32], 3);
        assert!(format!("{chain:?}").contains("redacted"));
    }

    #[test]
    fn chain_index_overflow_is_an_error() {
        let chain = ChainKey::new([0; 32], u32::MAX);
        assert!(chain.next().is_err());
    }
}
