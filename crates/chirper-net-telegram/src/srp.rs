//! The SRP proof behind the two-step verification password: `account.getPassword` gives the
//! parameters, `auth.checkPassword` takes the proof built here.
//!
//! Ported from gotd's `crypto/srp` (`hash.go`, `new_hash.go`, `srp.go`, MIT licence, Copyright
//! (c) 2020 Aleksandr Razumov). The hashes are SHA-256 and PBKDF2-HMAC-SHA512 with 100 000
//! rounds, as the algorithm `passwordKdfAlgoSHA256SHA256PBKDF2HMACSHA512iter100000SHA256ModPow`
//! requires. Only the parameter check and the modular arithmetic are done here; no primitive is
//! hand-rolled.

use num_bigint::BigUint;
use sha2::Sha512;

use crate::mtproto::crypto::{self, CryptoError};

/// PBKDF2 rounds of the password hash.
pub const PBKDF2_ROUNDS: u32 = 100_000;

/// The server's parameters for the password check.
#[derive(Clone, Copy, Debug)]
pub struct Params<'a> {
    pub salt1: &'a [u8],
    pub salt2: &'a [u8],
    pub g: i32,
    /// The prime, big-endian, as the server sent it.
    pub p: &'a [u8],
}

/// What `auth.checkPassword` needs: the client's public value and the proof.
#[derive(Clone, PartialEq, Eq)]
pub struct Proof {
    /// `g^a mod p`, 256 bytes.
    pub a: Vec<u8>,
    /// `M1`, 32 bytes.
    pub m1: Vec<u8>,
}

impl std::fmt::Debug for Proof {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Proof").finish_non_exhaustive()
    }
}

fn hash(parts: &[&[u8]]) -> [u8; 32] {
    crypto::sha256(parts)
}

/// `H(salt | data | salt)`.
fn salt_hash(data: &[u8], salt: &[u8]) -> [u8; 32] {
    hash(&[salt, data, salt])
}

/// The inner hash of the password: `H(salt2 | H(salt1 | password | salt1) | salt2)`.
fn primary(password: &[u8], salt1: &[u8], salt2: &[u8]) -> [u8; 32] {
    salt_hash(&salt_hash(password, salt1), salt2)
}

/// The private exponent `x`: `H(salt2 | PBKDF2(primary, salt1) | salt2)`.
fn secret_exponent(password: &[u8], salt1: &[u8], salt2: &[u8]) -> BigUint {
    let inner = primary(password, salt1, salt2);
    let mut stretched = [0u8; 64];
    pbkdf2::pbkdf2_hmac::<Sha512>(&inner, salt1, PBKDF2_ROUNDS, &mut stretched);
    BigUint::from_bytes_be(&salt_hash(&stretched, salt2))
}

/// `value` as exactly 256 big-endian bytes; fails when it does not fit.
fn pad256(value: &BigUint) -> Result<[u8; 256], CryptoError> {
    crypto::to_fixed_be(value, 256)?
        .try_into()
        .map_err(|_| CryptoError::OutOfRange)
}

/// The last 256 bytes of `bytes`, left-padded with zeros when shorter.
fn pad_bytes(bytes: &[u8]) -> [u8; 256] {
    let mut out = [0u8; 256];
    let take = bytes.len().min(256);
    let src = bytes.get(bytes.len() - take..).unwrap_or(&[]);
    let start = 256 - take;
    if let Some(dst) = out.get_mut(start..) {
        dst.copy_from_slice(src);
    }
    out
}

/// Build the proof for `password` against the server's `srp_b`, with the client's random 256
/// bytes `random` as the private value `a`.
pub fn prove(
    password: &[u8],
    params: Params<'_>,
    srp_b: &[u8],
    random: &[u8; 256],
) -> Result<Proof, CryptoError> {
    let p = crypto::check_dh_params(params.p, params.g)?;
    let g = BigUint::from(u32::try_from(params.g).map_err(|_| CryptoError::OutOfRange)?);
    let g_bytes = pad256(&g)?;

    let a = BigUint::from_bytes_be(random);
    let ga = g.modpow(&a, &p);
    let ga_bytes = pad256(&ga)?;

    let b = BigUint::from_bytes_be(srp_b);
    crypto::check_dh_value(&b, &p)?;
    let gb_bytes = pad_bytes(srp_b);

    let u = BigUint::from_bytes_be(&hash(&[&ga_bytes, &gb_bytes]));
    let x = secret_exponent(password, params.salt1, params.salt2);
    let v = g.modpow(&x, &p);
    let k = BigUint::from_bytes_be(&hash(&[params.p, &g_bytes]));
    let kv = (&k * &v) % &p;
    let t = (&b + &p - &kv) % &p;
    let exponent = &a + &u * &x;
    let sa = pad256(&t.modpow(&exponent, &p))?;
    let ka = hash(&[&sa]);

    let hp = hash(&[params.p]);
    let hg = hash(&[&g_bytes]);
    let mut xor_hp_hg = [0u8; 32];
    for ((out, l), r) in xor_hp_hg.iter_mut().zip(hp.iter()).zip(hg.iter()) {
        *out = l ^ r;
    }
    let m1 = hash(&[
        &xor_hp_hg,
        &hash(&[params.salt1]),
        &hash(&[params.salt2]),
        &ga_bytes,
        &gb_bytes,
        &ka,
    ]);
    Ok(Proof {
        a: ga_bytes.to_vec(),
        m1: m1.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decode a hex string written without separators.
    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .filter_map(|i| text.get(i..i + 2))
            .filter_map(|pair| u8::from_str_radix(pair, 16).ok())
            .collect()
    }

    /// The vector of gotd's `crypto/srp/srp_test.go` (MIT, Copyright (c) 2020 Aleksandr Razumov):
    /// password `123123`, random bytes all 0x01, expected A all 0x03.
    #[test]
    fn matches_gotd_vector() {
        let srp_b = hex(concat!(
            "9C52401A6A8084EC82F01C3725D3FB448BD2F0C909F9D97726EAC4B7A74172D9",
            "52F02466BE6734FA274D2B7429E27397F10372D66B400B80A5C5AE3F28B17BF3",
            "105D7A2D2A885998CDC2DEFC208AEC217AB58859A9ABC2374AD93DC285F4B3FB",
            "CAFF4143D7888F2425BD2FB711B25609CEB21757D935B1EF2F042173AD0CE2FE",
            "0E474DAC53914BD25A8A9AED4AEA8953D55CB88621DB37B871EA0D04393AC098",
            "7F68094CCC9DE8239251375D8FFFD263316CD528C097B7BC9FB919FBEDB76C52",
            "5DF3413C374EE076D97A1E6D352BB7CC80FD13651B04B32E2E48C5268150842C",
            "FD07CF855958B1B5EA9C36FDAD697FE3AEC8DCC6B1EFEC36874AF226204676CF"
        ));
        let salt1 =
            hex("4D11FB6BEC38F9D2546BB0F61E4F1C99A1BC0DB8F0D5F35B1291B37B213123D7ED48F3C6794D495B");
        let salt2 = hex("A1B181AAFE88188680AE32860D60BB01");
        let p = hex(concat!(
            "C71CAEB9C6B1C9048E6C522F70F13F73980D40238E3E21C14934D037563D930F",
            "48198A0AA7C14058229493D22530F4DBFA336F6E0AC925139543AED44CCE7C37",
            "20FD51F69458705AC68CD4FE6B6B13ABDC9746512969328454F18FAF8C595F64",
            "2477FE96BB2A941D5BCD1D4AC8CC49880708FA9B378E3C4F3A9060BEE67CF9A4",
            "A4A695811051907E162753B56B0F6B410DBA74D8A84B2A14B3144E0EF1284754",
            "FD17ED950D5965B4B9DD46582DB1178D169C6BC465B0D6FF9CA3928FEF5B9AE4",
            "E418FC15E83EBEA0F87FA9FF5EED70050DED2849F47BF959D956850CE929851F",
            "0D8115F635B105EE2E4E15D04B2454BF6F4FADF034B10403119CD8E3B92FCC5B"
        ));
        let params = Params {
            salt1: &salt1,
            salt2: &salt2,
            g: 3,
            p: &p,
        };
        // gotd's `setByte(256, 1)`: 256 zero bytes with 1 in the last four, big-endian.
        let mut random = [0u8; 256];
        if let Some(tail) = random.get_mut(252..) {
            tail.copy_from_slice(&1u32.to_be_bytes());
        }
        let proof = prove(b"123123", params, &srp_b, &random).unwrap();
        // A = g^a mod p = 3, as 256 big-endian bytes.
        let mut expected_a = [0u8; 256];
        if let Some(last) = expected_a.last_mut() {
            *last = 3;
        }
        assert_eq!(proof.a, expected_a.to_vec());
        assert_eq!(
            proof.m1,
            hex("999DF906BDA2C6CBB52F503406EBA2D0D0503ACE0CC302C38F13EE5010AD4051")
        );
    }

    #[test]
    fn rejects_a_foreign_prime() {
        let params = Params {
            salt1: &[1, 2],
            salt2: &[3, 4],
            g: 3,
            p: &[7u8; 256],
        };
        assert!(prove(b"pw", params, &[9u8; 256], &[1u8; 256]).is_err());
    }

    #[test]
    fn pad_bytes_keeps_the_last_256() {
        let long: Vec<u8> = (0..300u32).map(|i| i as u8).collect();
        let padded = pad_bytes(&long);
        assert_eq!(padded.first().copied(), long.get(44).copied());
        let short = pad_bytes(&[5, 6]);
        assert_eq!(short.get(254..), Some(&[5u8, 6][..]));
        assert_eq!(short.first(), Some(&0));
    }
}
