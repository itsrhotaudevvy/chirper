//! MTProto cryptography: AES-256-IGE, the MTProto 2.0 message keys, RSA_PAD, key fingerprints,
//! the `pq` factorisation and the Diffie-Hellman checks.
//!
//! The primitives (AES, SHA-1, SHA-256, big integers, the constant-time comparison) come from
//! RustCrypto. The protocol composition in this file follows the MTProto specification directly.

use aes::Aes256;
use aes::cipher::{Block, BlockCipherDecrypt, BlockCipherEncrypt, KeyInit};
use num_bigint::BigUint;
use sha1::{Digest as _, Sha1};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use super::constants::{DH_G, DH_PRIME, RSA_E, RSA_PROD_N};
use crate::tl;

/// Why a cryptographic step failed. None of these carry secret material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CryptoError {
    /// A buffer has the wrong length (not a multiple of 16, or too large).
    BadLength,
    /// The message key does not match the decrypted data.
    BadMsgKey,
    /// The operating system could not supply random bytes.
    Random,
    /// A value is outside the range the protocol allows.
    OutOfRange,
}

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadLength => write!(f, "bad buffer length"),
            Self::BadMsgKey => write!(f, "message key mismatch"),
            Self::Random => write!(f, "no random bytes available"),
            Self::OutOfRange => write!(f, "value out of range"),
        }
    }
}

impl std::error::Error for CryptoError {}

/// Fill a fixed-size array from the operating system's random source.
pub fn random_array<const N: usize>() -> Result<[u8; N], CryptoError> {
    let mut out = [0u8; N];
    getrandom::fill(&mut out).map_err(|_| CryptoError::Random)?;
    Ok(out)
}

/// SHA-1 over the concatenation of `parts`.
pub fn sha1(parts: &[&[u8]]) -> [u8; 20] {
    let mut hasher = Sha1::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// SHA-256 over the concatenation of `parts`.
pub fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

fn xor16(a: &[u8; 16], b: &[u8; 16]) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (o, (x, y)) in out.iter_mut().zip(a.iter().zip(b.iter())) {
        *o = x ^ y;
    }
    out
}

/// AES-256-IGE encryption in place. `iv` is the 32-byte IV: the first half is the previous
/// ciphertext block, the second half the previous plaintext block.
pub fn ige_encrypt(key: &[u8; 32], iv: &[u8; 32], data: &mut [u8]) -> Result<(), CryptoError> {
    let cipher = Aes256::new_from_slice(key).map_err(|_| CryptoError::BadLength)?;
    ige_encrypt_with(&cipher, iv, data)
}

/// AES-256-IGE decryption in place. See [`ige_encrypt`] for the IV.
pub fn ige_decrypt(key: &[u8; 32], iv: &[u8; 32], data: &mut [u8]) -> Result<(), CryptoError> {
    let cipher = Aes256::new_from_slice(key).map_err(|_| CryptoError::BadLength)?;
    ige_decrypt_with(&cipher, iv, data)
}

fn split_iv(iv: &[u8; 32]) -> ([u8; 16], [u8; 16]) {
    let mut first = [0u8; 16];
    let mut second = [0u8; 16];
    first.copy_from_slice(&iv[..16]);
    second.copy_from_slice(&iv[16..]);
    (first, second)
}

/// IGE over any 16-byte block cipher: `y_i = E(x_i ^ y_{i-1}) ^ x_{i-1}`.
fn ige_encrypt_with<C: BlockCipherEncrypt>(
    cipher: &C,
    iv: &[u8; 32],
    data: &mut [u8],
) -> Result<(), CryptoError> {
    if !data.len().is_multiple_of(16) {
        return Err(CryptoError::BadLength);
    }
    let (mut prev_cipher, mut prev_plain) = split_iv(iv);
    for chunk in data.as_chunks_mut::<16>().0 {
        let mut plain = [0u8; 16];
        plain.copy_from_slice(chunk);
        let mixed = xor16(&plain, &prev_cipher);
        let mut block = Block::<C>::default();
        block.copy_from_slice(&mixed);
        cipher.encrypt_block(&mut block);
        let mut out = [0u8; 16];
        out.copy_from_slice(&block);
        let out = xor16(&out, &prev_plain);
        chunk.copy_from_slice(&out);
        prev_cipher = out;
        prev_plain = plain;
    }
    Ok(())
}

/// IGE decryption: `x_i = D(y_i ^ x_{i-1}) ^ y_{i-1}`.
fn ige_decrypt_with<C: BlockCipherDecrypt>(
    cipher: &C,
    iv: &[u8; 32],
    data: &mut [u8],
) -> Result<(), CryptoError> {
    if !data.len().is_multiple_of(16) {
        return Err(CryptoError::BadLength);
    }
    let (mut prev_cipher, mut prev_plain) = split_iv(iv);
    for chunk in data.as_chunks_mut::<16>().0 {
        let mut cipher_block = [0u8; 16];
        cipher_block.copy_from_slice(chunk);
        let mixed = xor16(&cipher_block, &prev_plain);
        let mut block = Block::<C>::default();
        block.copy_from_slice(&mixed);
        cipher.decrypt_block(&mut block);
        let mut out = [0u8; 16];
        out.copy_from_slice(&block);
        let plain = xor16(&out, &prev_cipher);
        chunk.copy_from_slice(&plain);
        prev_cipher = cipher_block;
        prev_plain = plain;
    }
    Ok(())
}

/// The auth-key offset `x`: 0 for client-to-server, 8 for server-to-client messages.
pub const CLIENT_X: usize = 0;
pub const SERVER_X: usize = 8;

/// MTProto 2.0 `aes_key` and `aes_iv` for a message key.
pub fn message_aes(auth_key: &[u8; 256], x: usize, msg_key: &[u8; 16]) -> ([u8; 32], [u8; 32]) {
    let a = sha256(&[msg_key, &auth_key[x..x + 36]]);
    let b = sha256(&[&auth_key[40 + x..40 + x + 36], msg_key]);
    let mut key = [0u8; 32];
    key[..8].copy_from_slice(&a[..8]);
    key[8..24].copy_from_slice(&b[8..24]);
    key[24..].copy_from_slice(&a[24..32]);
    let mut iv = [0u8; 32];
    iv[..8].copy_from_slice(&b[..8]);
    iv[8..24].copy_from_slice(&a[8..24]);
    iv[24..].copy_from_slice(&b[24..32]);
    (key, iv)
}

/// The 128-bit `msg_key` of MTProto 2.0: the middle of `SHA256(auth_key part || plaintext)`.
pub fn message_key(auth_key: &[u8; 256], x: usize, plaintext: &[u8]) -> [u8; 16] {
    let large = sha256(&[&auth_key[88 + x..88 + x + 32], plaintext]);
    let mut key = [0u8; 16];
    key.copy_from_slice(&large[8..24]);
    key
}

/// Encrypt `plaintext` (which already includes its random padding) for an auth key. Returns the
/// `msg_key` to put in the transport header and the ciphertext.
pub fn encrypt_message(
    auth_key: &[u8; 256],
    x: usize,
    plaintext: &[u8],
) -> Result<([u8; 16], Vec<u8>), CryptoError> {
    if plaintext.is_empty() || !plaintext.len().is_multiple_of(16) {
        return Err(CryptoError::BadLength);
    }
    let msg_key = message_key(auth_key, x, plaintext);
    let (key, iv) = message_aes(auth_key, x, &msg_key);
    let mut data = plaintext.to_vec();
    ige_encrypt(&key, &iv, &mut data)?;
    Ok((msg_key, data))
}

/// Decrypt and authenticate a message. The message key is checked before any byte is returned.
pub fn decrypt_message(
    auth_key: &[u8; 256],
    x: usize,
    msg_key: &[u8; 16],
    ciphertext: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    if ciphertext.is_empty() || !ciphertext.len().is_multiple_of(16) {
        return Err(CryptoError::BadLength);
    }
    let (key, iv) = message_aes(auth_key, x, msg_key);
    let mut plain = ciphertext.to_vec();
    ige_decrypt(&key, &iv, &mut plain)?;
    let expected = message_key(auth_key, x, &plain);
    if bool::from(expected.ct_eq(msg_key)) {
        Ok(plain)
    } else {
        Err(CryptoError::BadMsgKey)
    }
}

/// Padding for an encrypted message of `data_len` bytes: at least 12 and at most 1024 bytes, and
/// the total a multiple of 16. `random` chooses how many extra 16-byte blocks to add.
pub fn padding_len(data_len: usize, random: u32) -> usize {
    let base = 12 + (16 - (data_len + 12) % 16) % 16;
    let extra_blocks = (1024 - base) / 16;
    let extra = (random as usize % (extra_blocks + 1)) * 16;
    base + extra
}

/// `auth_key_id`: the low 64 bits of `SHA1(auth_key)` read as a little-endian value.
pub fn auth_key_id(auth_key: &[u8; 256]) -> u64 {
    let digest = sha1(&[auth_key]);
    let mut id = [0u8; 8];
    id.copy_from_slice(&digest[12..20]);
    u64::from_le_bytes(id)
}

/// A server RSA public key.
#[derive(Clone, Debug)]
pub struct RsaPublicKey {
    pub n: BigUint,
    pub e: BigUint,
}

impl RsaPublicKey {
    /// The production key Telegram publishes.
    pub fn production() -> Self {
        Self {
            n: BigUint::parse_bytes(RSA_PROD_N.as_bytes(), 10).unwrap_or_default(),
            e: BigUint::from(RSA_E),
        }
    }
}

/// The key fingerprint: the low 64 bits of `SHA1(TL(n) || TL(e))`, as a signed little-endian value.
pub fn fingerprint(key: &RsaPublicKey) -> i64 {
    let mut buf = Vec::new();
    tl::write_bytes(&mut buf, &key.n.to_bytes_be());
    tl::write_bytes(&mut buf, &key.e.to_bytes_be());
    let digest = sha1(&[&buf]);
    let mut low = [0u8; 8];
    low.copy_from_slice(&digest[12..20]);
    i64::from_le_bytes(low)
}

/// RSA_PAD with caller-supplied randomness: `random` is 224 bytes, where the first `192 - len`
/// bytes pad the data and bytes 192..224 are the temp key. `next_key` supplies the next temp key
/// when the padded value is not smaller than the modulus; it receives the previous one.
pub fn rsa_pad_encrypt_with(
    data: &[u8],
    key: &RsaPublicKey,
    random: &[u8; 224],
    mut next_key: impl FnMut(&[u8; 32]) -> Result<[u8; 32], CryptoError>,
) -> Result<Vec<u8>, CryptoError> {
    if data.len() > 144 {
        return Err(CryptoError::BadLength);
    }
    let pad_len = 192 - data.len();
    let mut data_with_padding = Vec::with_capacity(192);
    data_with_padding.extend_from_slice(data);
    data_with_padding.extend_from_slice(&random[..pad_len]);
    let reversed: Vec<u8> = data_with_padding.iter().rev().copied().collect();
    let mut temp_key = [0u8; 32];
    temp_key.copy_from_slice(&random[192..224]);
    let key_aes_encrypted = loop {
        let hash = sha256(&[&temp_key, &data_with_padding]);
        let mut aes_encrypted = Vec::with_capacity(224);
        aes_encrypted.extend_from_slice(&reversed);
        aes_encrypted.extend_from_slice(&hash);
        ige_encrypt(&temp_key, &[0u8; 32], &mut aes_encrypted)?;
        let mask = sha256(&[&aes_encrypted]);
        let mut combined = Vec::with_capacity(256);
        combined.extend(temp_key.iter().zip(mask.iter()).map(|(a, b)| a ^ b));
        combined.extend_from_slice(&aes_encrypted);
        if BigUint::from_bytes_be(&combined) < key.n {
            break combined;
        }
        temp_key = next_key(&temp_key)?;
    };
    let payload = BigUint::from_bytes_be(&key_aes_encrypted);
    let encrypted = payload.modpow(&key.e, &key.n).to_bytes_be();
    let mut block = vec![0u8; 256 - encrypted.len().min(256)];
    block.extend_from_slice(&encrypted);
    Ok(block)
}

/// RSA_PAD with operating-system randomness.
pub fn rsa_pad_encrypt(data: &[u8], key: &RsaPublicKey) -> Result<Vec<u8>, CryptoError> {
    let random: [u8; 224] = random_array()?;
    rsa_pad_encrypt_with(data, key, &random, |_| random_array::<32>())
}

/// Factor `pq` into primes `p < q` with Pollard's rho (Brent's variant).
pub fn factorize(pq: u64) -> Option<(u64, u64)> {
    if pq < 4 {
        return None;
    }
    if pq.is_multiple_of(2) {
        return Some((2, pq / 2));
    }
    let mut c = 1u64;
    while c < 64 {
        if let Some(d) = brent(pq, c) {
            let (p, q) = (d.min(pq / d), d.max(pq / d));
            return Some((p, q));
        }
        c += 1;
    }
    None
}

fn mulmod(a: u64, b: u64, m: u64) -> u64 {
    ((u128::from(a) * u128::from(b)) % u128::from(m)) as u64
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

fn brent(n: u64, c: u64) -> Option<u64> {
    let f = |v: u64| (mulmod(v, v, n) + c) % n;
    let mut y: u64 = 2;
    let mut x: u64 = y;
    let mut ys: u64 = y;
    let mut g: u64 = 1;
    let mut q: u64 = 1;
    let m: u64 = 128;
    let mut r: u64 = 1;
    while g == 1 {
        x = y;
        for _ in 0..r {
            y = f(y);
        }
        let mut k: u64 = 0;
        while k < r && g == 1 {
            ys = y;
            let steps = m.min(r - k);
            for _ in 0..steps {
                y = f(y);
                q = mulmod(q, x.abs_diff(y), n);
            }
            g = gcd(q, n);
            k += m;
        }
        r *= 2;
        if r > (1u64 << 40) {
            return None;
        }
    }
    if g == n {
        loop {
            ys = f(ys);
            g = gcd(x.abs_diff(ys), n);
            if g > 1 {
                break;
            }
        }
    }
    if g == n || g == 1 { None } else { Some(g) }
}

/// Check the DH prime and generator from the server. Only the prime this client was built with
/// is accepted; any other prime is refused rather than tested for primality.
pub fn check_dh_params(prime: &[u8], g: i32) -> Result<BigUint, CryptoError> {
    if prime != DH_PRIME || g != DH_G {
        return Err(CryptoError::OutOfRange);
    }
    let p = BigUint::from_bytes_be(&DH_PRIME);
    let residue_ok = match g {
        2 => (&p % 8u32) == BigUint::from(7u32),
        3 => (&p % 3u32) == BigUint::from(2u32),
        4 => true,
        5 => matches!(u32::try_from(&p % 5u32), Ok(1) | Ok(4)),
        6 => matches!(u32::try_from(&p % 24u32), Ok(19) | Ok(23)),
        7 => matches!(u32::try_from(&p % 7u32), Ok(3) | Ok(5) | Ok(6)),
        _ => false,
    };
    if residue_ok {
        Ok(p)
    } else {
        Err(CryptoError::OutOfRange)
    }
}

/// `1 < value < p - 1` and `2^(2048-64) <= value <= p - 2^(2048-64)`.
pub fn check_dh_value(value: &BigUint, p: &BigUint) -> Result<(), CryptoError> {
    let margin = BigUint::from(1u32) << (2048 - 64);
    let one = BigUint::from(1u32);
    let upper = p - &one;
    let ok = *value > one && *value < upper && *value >= margin && *value <= (p - &margin);
    if ok {
        Ok(())
    } else {
        Err(CryptoError::OutOfRange)
    }
}

/// Big-endian bytes of a value, left-padded to exactly `len` bytes.
pub fn to_fixed_be(value: &BigUint, len: usize) -> Result<Vec<u8>, CryptoError> {
    let bytes = value.to_bytes_be();
    if bytes.len() > len {
        return Err(CryptoError::OutOfRange);
    }
    let mut out = vec![0u8; len - bytes.len()];
    out.extend_from_slice(&bytes);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        let digits: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        (0..digits.len())
            .step_by(2)
            .filter_map(|i| {
                digits
                    .get(i..i + 2)
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
            })
            .collect()
    }

    fn sequence_key() -> [u8; 256] {
        let mut key = [0u8; 256];
        for (i, b) in key.iter_mut().enumerate() {
            *b = i as u8;
        }
        key
    }

    #[test]
    fn ige_128_bit_vectors_from_mgp25() {
        // Vectors from https://mgp25.com/blog/2015/06/21/AESIGE/ (16-byte keys, 32-byte IVs). They
        // exercise the IGE chaining, which is independent of the key size.
        let vectors = [
            (
                "00010203 04050607 08090A0B 0C0D0E0F",
                "00010203 04050607 08090A0B 0C0D0E0F 10111213 14151617 18191A1B 1C1D1E1F",
                "00000000 00000000 00000000 00000000 00000000 00000000 00000000 00000000",
                "1A8519A6 557BE652 E9DA8E43 DA4EF445 3CF456B4 CA488AA3 83C79C98 B34797CB",
            ),
            (
                "54686973 20697320 616E2069 6D706C65",
                "6D656E74 6174696F 6E206F66 20494745 206D6F64 6520666F 72204F70 656E5353",
                "99706487 A1CDE613 BC6DE0B6 F24B1C7A A448C8B9 C3403E34 67A8CAD8 9340F53B",
                "4C2E204C 65742773 20686F70 65204265 6E20676F 74206974 20726967 6874210A",
            ),
        ];
        for (key, iv, plain, expected) in vectors {
            let key = hex(key);
            let iv: [u8; 32] = hex(iv).try_into().unwrap();
            let cipher = aes::Aes128::new_from_slice(&key).unwrap();
            let mut data = hex(plain);
            ige_encrypt_with(&cipher, &iv, &mut data).unwrap();
            assert_eq!(data, hex(expected));
            ige_decrypt_with(&cipher, &iv, &mut data).unwrap();
            assert_eq!(data, hex(plain));
        }
    }

    #[test]
    fn ige_single_block_with_zero_iv_is_plain_aes() {
        // FIPS-197 appendix C.3 (AES-256). With a zero IV an IGE single block is ECB of that block.
        let key = hex("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f");
        let key: [u8; 32] = key.try_into().unwrap();
        let mut data = hex("00112233445566778899aabbccddeeff");
        ige_encrypt(&key, &[0u8; 32], &mut data).unwrap();
        assert_eq!(data, hex("8ea2b7ca516745bfeafc49904b496089"));
        ige_decrypt(&key, &[0u8; 32], &mut data).unwrap();
        assert_eq!(data, hex("00112233445566778899aabbccddeeff"));
    }

    #[test]
    fn ige_256_round_trips_and_rejects_partial_blocks() {
        let key = [7u8; 32];
        let iv = [9u8; 32];
        let original: Vec<u8> = (0..128u8).collect();
        let mut data = original.clone();
        ige_encrypt(&key, &iv, &mut data).unwrap();
        assert_ne!(data, original);
        ige_decrypt(&key, &iv, &mut data).unwrap();
        assert_eq!(data, original);
        let mut odd = vec![0u8; 20];
        assert_eq!(
            ige_encrypt(&key, &iv, &mut odd),
            Err(CryptoError::BadLength)
        );
    }

    #[test]
    fn message_key_derivation_matches_published_vectors() {
        // Vectors from grammers-crypto 0.12.0 (MIT OR Apache-2.0), tests of calc_key: auth key is
        // the bytes 0..=255, msg_key is the bytes 0..=15.
        let key = sequence_key();
        let msg_key: [u8; 16] = std::array::from_fn(|i| i as u8);
        let (aes_key, aes_iv) = message_aes(&key, CLIENT_X, &msg_key);
        assert_eq!(
            aes_key,
            [
                112, 78, 208, 156, 139, 65, 102, 138, 232, 249, 157, 36, 71, 56, 247, 29, 189, 220,
                68, 70, 155, 107, 189, 74, 168, 87, 61, 208, 66, 189, 5, 158
            ]
        );
        assert_eq!(
            aes_iv,
            [
                77, 38, 96, 0, 165, 80, 237, 171, 191, 76, 124, 228, 15, 208, 4, 60, 201, 34, 48,
                24, 76, 211, 23, 165, 204, 156, 36, 130, 253, 59, 147, 24
            ]
        );
        let (server_key, server_iv) = message_aes(&key, SERVER_X, &msg_key);
        assert_eq!(
            server_key,
            [
                33, 119, 37, 121, 155, 36, 88, 6, 69, 129, 116, 161, 252, 251, 200, 131, 144, 104,
                7, 177, 80, 51, 253, 208, 234, 43, 77, 105, 207, 156, 54, 78
            ]
        );
        assert_eq!(
            server_iv,
            [
                102, 154, 101, 56, 145, 122, 79, 165, 108, 163, 35, 96, 164, 49, 201, 22, 11, 228,
                173, 136, 113, 64, 152, 13, 171, 145, 206, 123, 220, 71, 255, 188
            ]
        );
    }

    #[test]
    fn padding_is_in_range_and_aligns_the_total() {
        for data_len in (0..=1100).step_by(4) {
            for random in [0u32, 1, 7, 1000, u32::MAX] {
                let pad = padding_len(data_len, random);
                assert!((12..=1024).contains(&pad), "pad {pad} for {data_len}");
                assert_eq!((data_len + pad) % 16, 0);
            }
        }
    }

    #[test]
    fn encrypt_then_decrypt_round_trips_and_detects_tampering() {
        let key = sequence_key();
        let plain: Vec<u8> = (0..64u8).collect();
        let (msg_key, cipher) = encrypt_message(&key, CLIENT_X, &plain).unwrap();
        let back = decrypt_message(&key, CLIENT_X, &msg_key, &cipher).unwrap();
        assert_eq!(back, plain);
        let mut bad_key = msg_key;
        bad_key[0] ^= 1;
        assert_eq!(
            decrypt_message(&key, CLIENT_X, &bad_key, &cipher),
            Err(CryptoError::BadMsgKey)
        );
        // The wrong direction (x) must not authenticate either.
        assert_eq!(
            decrypt_message(&key, SERVER_X, &msg_key, &cipher),
            Err(CryptoError::BadMsgKey)
        );
    }

    #[test]
    fn auth_key_id_is_a_little_endian_sha1_slice() {
        let key = sequence_key();
        let digest = sha1(&[&key]);
        let mut expected = [0u8; 8];
        expected.copy_from_slice(&digest[12..20]);
        assert_eq!(auth_key_id(&key), u64::from_le_bytes(expected));
    }

    #[test]
    fn production_key_fingerprint_matches_the_published_value() {
        let key = RsaPublicKey::production();
        assert_eq!(
            fingerprint(&key),
            super::super::constants::RSA_PROD_FINGERPRINT
        );
    }

    /// Telegram test-network RSA modulus (grammers-mtproto 0.12.0, authentication.rs).
    const TEST_KEY_N: &str = "25342889448840415564971689590713473206898847759084779052582026594546022463853940585885215951168491965708222649399180603818074200620463776135424884632162512403163793083921641631564740959529419359595852941166848940585952337613333022396096584117954892216031229237302943701877588456738335398602461675225081791820393153757504952636234951323237820036543581047826906120927972487366805292115792231423684261262330394324750785450942589751755390156647751460719351439969059949569615302809050721500330239005077889855323917509948255722081644689442127297605422579707142646660768825302832201908302295573257427896031830742328565032949";

    #[test]
    fn rsa_pad_matches_the_published_example() {
        // Example from grammers-crypto 0.12.0 (MIT OR Apache-2.0), rsa.rs test_rsa_encryption. It
        // uses Telegram's test-network key, which grammers lists beside the production one.
        let key = RsaPublicKey {
            n: BigUint::parse_bytes(TEST_KEY_N.as_bytes(), 10).unwrap(),
            e: BigUint::from(65537u32),
        };
        let data = hex(
            "955ff5a9081a8e635f5743de9b00000004453dc27100000004622f1fcb000000f7a81627bbf511fa4afef71e94a0937474586c1add9198dda81a5df8393871c8293623c5fb968894af1be7dfe9c7be813f9307789242fd0cb0c16a5cb39a8d3e",
        );
        let random = hex(
            "12270000635593b03fee033d0672f9afddf9124de9e77df6251806cba93482e4c9e6e06e7d44e4c4baae821aff91af44789689faaee9bdfc7b2df8c08709afe57396c4638ceaa0dc30114f82447e81d3b53edc423b32660c43a5b8ad057b64500000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000007dada0920c4973913229e0f881aec7b9db0c392d34f52fb0995ea493ecb4c09d",
        );
        let random: [u8; 224] = random.try_into().unwrap();
        // The published example needs one retry; the retry increments the temp key, as grammers does.
        let increment = |prev: &[u8; 32]| {
            let mut next = *prev;
            for byte in next.iter_mut().rev() {
                let (value, carry) = byte.overflowing_add(1);
                *byte = value;
                if !carry {
                    break;
                }
            }
            Ok(next)
        };
        let out = rsa_pad_encrypt_with(&data, &key, &random, increment).unwrap();
        let expected = hex(
            "b610642a828b4a61fe32931815cae318d311660580f1e0df768f3140f4d37dfcfcac0c2870318de4ff2d2e0e9669bcfdc0bad06cadb1b59d9726b427368a9c7b4fc0d5e7b2e99fc571968705c03acf5341fd7021bef653fa77b3776ae430e366fc46d232459ebe128b08d80e049ae579a48b56ca93b520709468587c81af96666046e9ea85091d729e921e8d8a36f57b27644052dae7387c7f4131701d59cda75251dac66c94276280ef950d3c44c21e5a2454f7da7a6818cf23ae9c490b72b2170d7cbc24f8a93db739d76f2d241c78b80123faaff3e664f074d6375d794dbf2800a0b5bb48d54eceafedfb355bfbebd287d9023264e3b53627888250787a9e",
        );
        assert_eq!(out, expected);
    }

    #[test]
    fn factorises_the_published_pq() {
        // Example from the MTProto auth_key documentation: pq = 0x17ED48941A08F981.
        let pq = 0x17ED_4894_1A08_F981u64;
        assert_eq!(factorize(pq), Some((0x494C_553B, 0x5391_1073)));
    }

    #[test]
    fn factorises_small_and_even_values() {
        assert_eq!(factorize(3), None);
        assert_eq!(factorize(10), Some((2, 5)));
        assert_eq!(factorize(15), Some((3, 5)));
        assert_eq!(
            factorize(1_000_003u64 * 1_000_033),
            Some((1_000_003, 1_000_033))
        );
    }

    #[test]
    fn dh_prime_is_accepted_only_as_built_in_and_generator_residues_hold() {
        let p = check_dh_params(&DH_PRIME, DH_G).unwrap();
        assert_eq!(p.bits(), 2048);
        let mut other = DH_PRIME;
        other[255] ^= 2;
        assert_eq!(check_dh_params(&other, DH_G), Err(CryptoError::OutOfRange));
        assert_eq!(check_dh_params(&DH_PRIME, 9), Err(CryptoError::OutOfRange));
    }

    #[test]
    fn built_in_dh_prime_is_a_safe_prime() {
        // Miller-Rabin with fixed bases on p and (p-1)/2. Slow in debug builds; that is accepted.
        fn probably_prime(n: &BigUint) -> bool {
            let one = BigUint::from(1u32);
            let two = BigUint::from(2u32);
            let n_minus_one = n - &one;
            let mut d = n_minus_one.clone();
            let mut s = 0u32;
            while (&d % &two) == BigUint::from(0u32) {
                d /= &two;
                s += 1;
            }
            'bases: for base in [2u32, 3, 5, 7, 11] {
                let a = BigUint::from(base);
                let mut x = a.modpow(&d, n);
                if x == one || x == n_minus_one {
                    continue;
                }
                for _ in 1..s {
                    x = x.modpow(&two, n);
                    if x == n_minus_one {
                        continue 'bases;
                    }
                }
                return false;
            }
            true
        }
        let p = BigUint::from_bytes_be(&DH_PRIME);
        let q = (&p - BigUint::from(1u32)) / BigUint::from(2u32);
        assert!(probably_prime(&p));
        assert!(probably_prime(&q));
    }

    #[test]
    fn dh_value_range_checks() {
        let p = BigUint::from_bytes_be(&DH_PRIME);
        let margin = BigUint::from(1u32) << (2048 - 64);
        assert!(check_dh_value(&(&margin + BigUint::from(1u32)), &p).is_ok());
        assert!(check_dh_value(&BigUint::from(1u32), &p).is_err());
        assert!(check_dh_value(&BigUint::from(2u32), &p).is_err());
        assert!(check_dh_value(&(&p - BigUint::from(1u32)), &p).is_err());
        assert!(check_dh_value(&(&p - &margin + BigUint::from(1u32)), &p).is_err());
    }
}
