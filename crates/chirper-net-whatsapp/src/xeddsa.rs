//! XEdDSA: signatures made with a Curve25519 (X25519) private key and checked against the matching
//! public key, as the Signal specification defines them (<https://signal.org/docs/specifications/xeddsa/>).
//!
//! WhatsApp signs its certificates, and the phone signs pairing identities, with keys of this kind.
//! This follows the variant those peers use: the sign bit of the Edwards form of the public key
//! travels in the most significant bit of the signature's last byte, and the verifier recovers the
//! Edwards point from the Montgomery `u` coordinate plus that bit.

use curve25519_dalek::constants::ED25519_BASEPOINT_POINT;
use curve25519_dalek::montgomery::MontgomeryPoint;
use curve25519_dalek::scalar::Scalar;
use sha2::{Digest, Sha512};

/// Length of an XEdDSA signature.
pub const SIGNATURE_LEN: usize = 64;

/// The XEdDSA hash prefix: `0xFE` followed by 31 bytes of `0xFF`, which keeps the nonce hash apart
/// from every other SHA-512 use.
const HASH_PREFIX: [u8; 32] = {
    let mut prefix = [0xFF; 32];
    prefix[0] = 0xFE;
    prefix
};

/// Clamp a 32-byte X25519 private scalar the way X25519 does.
pub fn clamp(mut bytes: [u8; 32]) -> [u8; 32] {
    bytes[0] &= 248;
    bytes[31] &= 127;
    bytes[31] |= 64;
    bytes
}

/// Derive the Montgomery `u` public key of a private scalar.
pub fn public_key(private: &[u8; 32]) -> [u8; 32] {
    let scalar = Scalar::from_bytes_mod_order(clamp(*private));
    let point = ED25519_BASEPOINT_POINT * scalar;
    point.to_montgomery().to_bytes()
}

/// Sign the concatenation of `parts` with `private`, using `random` as the 64 bytes of nonce
/// randomness. Callers draw `random` from the OS source; tests pass fixed bytes.
pub fn sign_with_random(private: &[u8; 32], parts: &[&[u8]], random: &[u8; 64]) -> [u8; 64] {
    let clamped = clamp(*private);
    let k = Scalar::from_bytes_mod_order(clamped);
    let edwards_public = ED25519_BASEPOINT_POINT * k;
    let edwards_bytes = edwards_public.compress().to_bytes();
    let sign_bit = edwards_bytes[31] & 0x80;

    // r = H(prefix || k || M || Z) mod q, R = rB.
    let mut nonce_hash = Sha512::new();
    nonce_hash.update(HASH_PREFIX);
    nonce_hash.update(clamped);
    for part in parts {
        nonce_hash.update(part);
    }
    nonce_hash.update(random);
    let r = Scalar::from_bytes_mod_order_wide(&nonce_hash.finalize().into());
    let big_r = (ED25519_BASEPOINT_POINT * r).compress().to_bytes();

    // h = H(R || A || M) mod q, s = h*k + r.
    let h = challenge(&big_r, &edwards_bytes, parts);
    let s = h * k + r;

    let mut signature = [0u8; 64];
    signature[..32].copy_from_slice(&big_r);
    signature[32..].copy_from_slice(&s.to_bytes());
    signature[63] = (signature[63] & 0x7F) | sign_bit;
    signature
}

/// Verify an XEdDSA signature over the concatenation of `parts` against the Montgomery public key
/// `public`. Malformed keys and signatures are rejected, never panicked on.
pub fn verify(public: &[u8; 32], parts: &[&[u8]], signature: &[u8; 64]) -> bool {
    let sign_bit = signature[63] >> 7;
    let Some(edwards) = MontgomeryPoint(*public).to_edwards(sign_bit) else {
        return false;
    };
    let edwards_bytes = edwards.compress().to_bytes();

    let mut big_r = [0u8; 32];
    big_r.copy_from_slice(&signature[..32]);
    let mut s_bytes = [0u8; 32];
    s_bytes.copy_from_slice(&signature[32..]);
    s_bytes[31] &= 0x7F;
    // The scalar must be below 2^253; anything above is not a value a signer produces.
    if s_bytes[31] & 0xE0 != 0 {
        return false;
    }
    let s = Scalar::from_bytes_mod_order(s_bytes);
    let h = challenge(&big_r, &edwards_bytes, parts);

    // R' = sB - hA, which must equal R for a valid signature.
    let check = curve25519_dalek::edwards::EdwardsPoint::vartime_double_scalar_mul_basepoint(
        &h,
        &(-edwards),
        &s,
    );
    check.compress().to_bytes() == big_r
}

fn challenge(big_r: &[u8; 32], public: &[u8; 32], parts: &[&[u8]]) -> Scalar {
    let mut hash = Sha512::new();
    hash.update(big_r);
    hash.update(public);
    for part in parts {
        hash.update(part);
    }
    Scalar::from_bytes_mod_order_wide(&hash.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> [u8; 32] {
        let mut bytes = [0u8; 32];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = seed.wrapping_mul(31).wrapping_add(i as u8).wrapping_mul(17);
        }
        bytes
    }

    #[test]
    fn sign_then_verify_round_trips_for_both_sign_bits() {
        let mut seen = [false; 2];
        for seed in 0..40u8 {
            let private = key(seed);
            let public = public_key(&private);
            let random = [seed; 64];
            let signature = sign_with_random(&private, &[b"hello", b" world"], &random);
            seen[usize::from(signature[63] >> 7)] = true;
            assert!(verify(&public, &[b"hello world"], &signature));
            assert!(verify(&public, &[b"hello", b" ", b"world"], &signature));
        }
        assert!(seen[0] && seen[1], "both sign bits must be exercised");
    }

    #[test]
    fn tampered_message_or_signature_fails() {
        let private = key(3);
        let public = public_key(&private);
        let signature = sign_with_random(&private, &[b"message"], &[9; 64]);
        assert!(verify(&public, &[b"message"], &signature));
        assert!(!verify(&public, &[b"messagf"], &signature));

        let mut flipped = signature;
        flipped[10] ^= 0x01;
        assert!(!verify(&public, &[b"message"], &flipped));

        let mut flipped_sign = signature;
        flipped_sign[63] ^= 0x80;
        assert!(!verify(&public, &[b"message"], &flipped_sign));
    }

    #[test]
    fn wrong_key_fails() {
        let signature = sign_with_random(&key(1), &[b"m"], &[2; 64]);
        assert!(!verify(&public_key(&key(2)), &[b"m"], &signature));
    }

    #[test]
    fn oversized_scalar_is_rejected() {
        let private = key(4);
        let public = public_key(&private);
        let mut signature = sign_with_random(&private, &[b"m"], &[5; 64]);
        signature[62] = 0xFF;
        assert!(!verify(&public, &[b"m"], &signature));
    }

    #[test]
    fn clamp_is_idempotent_and_matches_x25519_rules() {
        let clamped = clamp([0xFF; 32]);
        assert_eq!(clamped[0] & 7, 0);
        assert_eq!(clamped[31] & 0x80, 0);
        assert_eq!(clamped[31] & 0x40, 0x40);
        assert_eq!(clamp(clamped), clamped);
    }

    /// A real certificate: the intermediate certificate of a WhatsApp server's Noise chain, captured
    /// from a live handshake. Its signature was made by the WhatsApp root key that wacore-noise 0.7.0
    /// (MIT) ships, so this checks the verifier against signatures the network itself produces. The
    /// reference crate's tests carry no fixed XEdDSA triple, hence this one.
    #[test]
    fn verifies_the_live_intermediate_certificate_under_the_whatsapp_root() {
        let root: [u8; 32] = [
            0x14, 0x23, 0x75, 0x57, 0x4d, 0x0a, 0x58, 0x71, 0x66, 0xaa, 0xe7, 0x1e, 0xbe, 0x51,
            0x64, 0x37, 0xc4, 0xa2, 0x8b, 0x73, 0xe3, 0x69, 0x5c, 0x6c, 0xe1, 0xf7, 0xf9, 0x54,
            0x5d, 0xa8, 0xee, 0x6b,
        ];
        let details = hex::decode("080310001a201c51a9ac303994c6c8d0b92ea1878a533476599cc599fbea35997d9aa90cce62208091aebe0628ffdeb7dc06").expect("hex");
        let signature: [u8; 64] = hex::decode("270f294648539fed4870e25054dd4e95983aba29189c2ba6c8eeda7055555f753740f5ec192ab64c26c26d6ade6d20b9f774aee37120a6b20395f53c66058507")
            .expect("hex")
            .try_into()
            .expect("64 bytes");
        assert!(verify(&root, &[&details], &signature));
        let mut tampered = details.clone();
        tampered[0] ^= 1;
        assert!(!verify(&root, &[&tampered], &signature));
    }
}
