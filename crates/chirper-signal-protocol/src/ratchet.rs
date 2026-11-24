//! Initial key agreement: X3DH (version 3) and PQXDH (version 4), for both roles.
//!
//! Port of libsignal `ratchet/Ratchet.go` (`CalculateSenderSession`, `CalculateReceiverSession`)
//! and the PQXDH extension of the Rust implementation (AGPL-3.0-only, Signal Messenger LLC).
//!
//! The master secret is `0xFF × 32 || DH1 || DH2 || DH3 [|| DH4] [|| KEM secret]`, from which
//! the root key and the first chain key are derived with HKDF-SHA256, no salt, and the
//! version's info string (`"WhisperText"` or the PQXDH info).

use zeroize::Zeroizing;

use crate::curve::{KeyPair, PublicKey};
use crate::error::{Error, Result};
use crate::identity::{IdentityKey, IdentityKeyPair};
use crate::kdf::{ChainKey, RootKey, derive_root_and_chain};
use crate::protocol::CiphertextMessageType;

/// Message version of a session without a Kyber prekey (X3DH).
pub const VERSION_X3DH: u32 = 3;
/// Message version of a session with a Kyber prekey (PQXDH).
pub const VERSION_PQXDH: u32 = 4;
/// HKDF info for X3DH sessions.
const INFO_X3DH: &[u8] = b"WhisperText";
/// HKDF info for PQXDH sessions.
const INFO_PQXDH: &[u8] = b"WhisperText_X25519_SHA-256_CRYSTALS-KYBER-1024";
/// The 32-byte `0xFF` prefix that keeps the master secret away from any valid DH output.
const DISCONTINUITY: [u8; 32] = [0xFF; 32];

/// Inputs for the initiating side (Alice), who has fetched Bob's prekey bundle.
pub struct AliceParameters<'a> {
    /// Alice's identity key pair.
    pub our_identity: &'a IdentityKeyPair,
    /// Alice's ephemeral base key pair for this session.
    pub our_base_key: &'a KeyPair,
    /// Bob's identity key.
    pub their_identity: &'a IdentityKey,
    /// Bob's signed prekey.
    pub their_signed_pre_key: &'a PublicKey,
    /// Bob's one-time prekey, if the bundle had one.
    pub their_one_time_pre_key: Option<&'a PublicKey>,
    /// The KEM shared secret from encapsulating to Bob's Kyber prekey (PQXDH only).
    pub kyber_shared_secret: Option<&'a [u8]>,
}

/// Inputs for the responding side (Bob), who received a prekey message.
pub struct BobParameters<'a> {
    /// Bob's identity key pair.
    pub our_identity: &'a IdentityKeyPair,
    /// Bob's signed prekey pair, named in the prekey message.
    pub our_signed_pre_key: &'a KeyPair,
    /// Bob's one-time prekey pair, if the message used one.
    pub our_one_time_pre_key: Option<&'a KeyPair>,
    /// Alice's identity key.
    pub their_identity: &'a IdentityKey,
    /// Alice's base key from the prekey message.
    pub their_base_key: &'a PublicKey,
    /// The KEM shared secret from decapsulating Alice's ciphertext (PQXDH only).
    pub kyber_shared_secret: Option<&'a [u8]>,
}

/// The initial root key and the first chain key of a session.
#[derive(Debug, Clone)]
pub struct InitialKeys {
    /// Root key after the initial derivation.
    pub root_key: RootKey,
    /// Chain key at index 0.
    pub chain_key: ChainKey,
}

/// The message version a session uses: 4 when a KEM secret is present, 3 otherwise.
pub fn version_for(kyber_shared_secret: Option<&[u8]>) -> u32 {
    if kyber_shared_secret.is_some() {
        VERSION_PQXDH
    } else {
        VERSION_X3DH
    }
}

/// Checks that the KEM secret matches the session version and returns the HKDF info.
fn info_for(version: u32, kyber_shared_secret: Option<&[u8]>) -> Result<&'static [u8]> {
    match (version, kyber_shared_secret) {
        (VERSION_X3DH, None) => Ok(INFO_X3DH),
        (VERSION_PQXDH, Some(_)) => Ok(INFO_PQXDH),
        _ => Err(Error::InvalidArgument(
            "kyber secret does not match session version",
        )),
    }
}

/// Derives the initial keys on the initiating side.
///
/// DH1 = `IK_A × SPK_B`, DH2 = `EK_A × IK_B`, DH3 = `EK_A × SPK_B`, DH4 = `EK_A × OPK_B` (if any).
pub fn initialize_alice_session(params: &AliceParameters<'_>, version: u32) -> Result<InitialKeys> {
    let info = info_for(version, params.kyber_shared_secret)?;
    let mut secrets = Zeroizing::new(Vec::with_capacity(32 * 6 + 32));
    secrets.extend_from_slice(&DISCONTINUITY);
    secrets.extend_from_slice(
        &params
            .our_identity
            .private_key()
            .calculate_agreement(params.their_signed_pre_key)?[..],
    );
    secrets.extend_from_slice(
        &params
            .our_base_key
            .private_key
            .calculate_agreement(params.their_identity.public_key())?[..],
    );
    secrets.extend_from_slice(
        &params
            .our_base_key
            .private_key
            .calculate_agreement(params.their_signed_pre_key)?[..],
    );
    if let Some(one_time) = params.their_one_time_pre_key {
        secrets.extend_from_slice(
            &params
                .our_base_key
                .private_key
                .calculate_agreement(one_time)?[..],
        );
    }
    if let Some(kem) = params.kyber_shared_secret {
        secrets.extend_from_slice(kem);
    }
    let (root_key, chain_key) = derive_root_and_chain(&secrets, info)?;
    Ok(InitialKeys {
        root_key,
        chain_key,
    })
}

/// Derives the initial keys on the responding side. Mirrors [`initialize_alice_session`]:
///
/// DH1 = `SPK_B × IK_A`, DH2 = `IK_B × EK_A`, DH3 = `SPK_B × EK_A`, DH4 = `OPK_B × EK_A` (if any).
pub fn initialize_bob_session(params: &BobParameters<'_>, version: u32) -> Result<InitialKeys> {
    // Rejects small-order and high-bit base keys, as libsignal's pqxdh_accept does.
    if !params.their_base_key.is_canonical() {
        return Err(Error::InvalidMessage(
            CiphertextMessageType::PreKey,
            "incoming base key is invalid",
        ));
    }
    let info = info_for(version, params.kyber_shared_secret)?;
    let mut secrets = Zeroizing::new(Vec::with_capacity(32 * 6 + 32));
    secrets.extend_from_slice(&DISCONTINUITY);
    secrets.extend_from_slice(
        &params
            .our_signed_pre_key
            .private_key
            .calculate_agreement(params.their_identity.public_key())?[..],
    );
    secrets.extend_from_slice(
        &params
            .our_identity
            .private_key()
            .calculate_agreement(params.their_base_key)?[..],
    );
    secrets.extend_from_slice(
        &params
            .our_signed_pre_key
            .private_key
            .calculate_agreement(params.their_base_key)?[..],
    );
    if let Some(one_time) = params.our_one_time_pre_key {
        secrets.extend_from_slice(
            &one_time
                .private_key
                .calculate_agreement(params.their_base_key)?[..],
        );
    }
    if let Some(kem) = params.kyber_shared_secret {
        secrets.extend_from_slice(kem);
    }
    let (root_key, chain_key) = derive_root_and_chain(&secrets, info)?;
    Ok(InitialKeys {
        root_key,
        chain_key,
    })
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    use super::*;

    #[test]
    fn x3dh_is_symmetric_with_and_without_one_time_key() {
        let mut rng = StdRng::seed_from_u64(10);
        let alice_id = IdentityKeyPair::generate(&mut rng);
        let bob_id = IdentityKeyPair::generate(&mut rng);
        let base = KeyPair::generate(&mut rng);
        let signed = KeyPair::generate(&mut rng);
        let one_time = KeyPair::generate(&mut rng);
        for opk in [None, Some(&one_time)] {
            let alice = initialize_alice_session(
                &AliceParameters {
                    our_identity: &alice_id,
                    our_base_key: &base,
                    their_identity: &bob_id.identity_key(),
                    their_signed_pre_key: &signed.public_key,
                    their_one_time_pre_key: opk.map(|k| &k.public_key),
                    kyber_shared_secret: None,
                },
                VERSION_X3DH,
            )
            .unwrap();
            let bob = initialize_bob_session(
                &BobParameters {
                    our_identity: &bob_id,
                    our_signed_pre_key: &signed,
                    our_one_time_pre_key: opk,
                    their_identity: &alice_id.identity_key(),
                    their_base_key: &base.public_key,
                    kyber_shared_secret: None,
                },
                VERSION_X3DH,
            )
            .unwrap();
            assert_eq!(alice.root_key.bytes(), bob.root_key.bytes());
            assert_eq!(alice.chain_key.bytes(), bob.chain_key.bytes());
        }
    }

    #[test]
    fn pqxdh_mixes_the_kem_secret_and_uses_its_info() {
        let mut rng = StdRng::seed_from_u64(11);
        let alice_id = IdentityKeyPair::generate(&mut rng);
        let bob_id = IdentityKeyPair::generate(&mut rng);
        let base = KeyPair::generate(&mut rng);
        let signed = KeyPair::generate(&mut rng);
        let kem_secret = [0x5au8; 32];
        let alice = |secret: Option<&[u8]>, version: u32| {
            initialize_alice_session(
                &AliceParameters {
                    our_identity: &alice_id,
                    our_base_key: &base,
                    their_identity: &bob_id.identity_key(),
                    their_signed_pre_key: &signed.public_key,
                    their_one_time_pre_key: None,
                    kyber_shared_secret: secret,
                },
                version,
            )
        };
        let with_kem = alice(Some(&kem_secret), VERSION_PQXDH).unwrap();
        let bob = initialize_bob_session(
            &BobParameters {
                our_identity: &bob_id,
                our_signed_pre_key: &signed,
                our_one_time_pre_key: None,
                their_identity: &alice_id.identity_key(),
                their_base_key: &base.public_key,
                kyber_shared_secret: Some(&kem_secret),
            },
            VERSION_PQXDH,
        )
        .unwrap();
        assert_eq!(with_kem.root_key.bytes(), bob.root_key.bytes());
        // Dropping the KEM secret, or using the wrong version, must not produce the same keys.
        let without = alice(None, VERSION_X3DH).unwrap();
        assert_ne!(with_kem.root_key.bytes(), without.root_key.bytes());
        assert!(alice(None, VERSION_PQXDH).is_err());
        assert!(alice(Some(&kem_secret), VERSION_X3DH).is_err());
    }

    #[test]
    fn bob_rejects_torsioned_and_high_bit_base_keys() {
        // Ported in spirit from libsignal tests/ratchet.rs (`test_bob_rejects_torsioned_basekey`
        // and `test_bob_rejects_highbit_basekey`).
        let mut rng = StdRng::seed_from_u64(12);
        let bob_id = IdentityKeyPair::generate(&mut rng);
        let alice_id = IdentityKeyPair::generate(&mut rng);
        let signed = KeyPair::generate(&mut rng);
        let bad_keys = [[0u8; 32], [0xFFu8; 32]];
        for raw in bad_keys {
            let mut bytes = vec![0x05];
            bytes.extend_from_slice(&raw);
            let base = PublicKey::deserialize(&bytes).unwrap();
            let result = initialize_bob_session(
                &BobParameters {
                    our_identity: &bob_id,
                    our_signed_pre_key: &signed,
                    our_one_time_pre_key: None,
                    their_identity: &alice_id.identity_key(),
                    their_base_key: &base,
                    kyber_shared_secret: None,
                },
                VERSION_X3DH,
            );
            assert!(result.is_err(), "base key {} accepted", hex::encode(raw));
        }
    }

    #[test]
    fn version_for_follows_kem_presence() {
        assert_eq!(version_for(None), VERSION_X3DH);
        assert_eq!(version_for(Some(&[0u8; 32])), VERSION_PQXDH);
    }
}
