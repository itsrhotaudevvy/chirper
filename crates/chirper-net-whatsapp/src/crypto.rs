//! The symmetric and key-derivation steps the WhatsApp wire formats use, in one place so every call
//! site is easy to audit. HKDF, HMAC-SHA256 and AES-256-CBC come from `chirper-signal-protocol`
//! (the same primitives the Signal layer uses); the rest are RustCrypto primitives. Nothing here
//! implements a cipher or a hash itself.
//!
//! Media keys and the file layout follow whatsmeow's `download.go`, `upload.go` and
//! `util/cbcutil` (Mozilla Public License 2.0): a 112-byte HKDF expansion of the media key gives
//! the IV, the AES key, the MAC key and a reference key; the file is AES-256-CBC over PKCS#7 with
//! a 10-byte truncated HMAC-SHA256 over `iv || ciphertext` appended.

use aes::cipher::{KeyIvInit, StreamCipher};
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use chirper_bridge::{Error, Result};
use chirper_signal_protocol::cipher::{aes_cbc_decrypt, aes_cbc_encrypt};
use chirper_signal_protocol::kdf::{hkdf_sha256, hmac_sha256};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::{Digest, Sha256, Sha512};

/// Largest media file a driver downloads or uploads.
pub const MAX_MEDIA: usize = 100 << 20;

/// The length of the MAC every encrypted media file ends with.
pub const MEDIA_MAC_LENGTH: usize = 10;

/// SHA-256 of `data`.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// HKDF-SHA256 with a zero salt, `len` bytes.
pub fn hkdf(ikm: &[u8], info: &[u8], len: usize) -> Result<Vec<u8>> {
    hkdf_sha256(ikm, None, info, len)
        .map(|out| out.to_vec())
        .map_err(|_| Error::Internal("hkdf failed".to_owned()))
}

/// HKDF-SHA256 with a salt, `len` bytes.
pub fn hkdf_salted(ikm: &[u8], salt: &[u8], info: &[u8], len: usize) -> Result<Vec<u8>> {
    hkdf_sha256(ikm, Some(salt), info, len)
        .map(|out| out.to_vec())
        .map_err(|_| Error::Internal("hkdf failed".to_owned()))
}

/// HMAC-SHA256 over the concatenation of `parts`.
pub fn hmac256(key: &[u8], parts: &[&[u8]]) -> Result<[u8; 32]> {
    hmac_sha256(key, parts)
        .map(|mac| *mac)
        .map_err(|_| Error::Internal("hmac failed".to_owned()))
}

/// HMAC-SHA512 over the concatenation of `parts`.
pub fn hmac512(key: &[u8], parts: &[&[u8]]) -> Result<[u8; 64]> {
    let mut mac = <Hmac<Sha512> as Mac>::new_from_slice(key)
        .map_err(|_| Error::Internal("hmac key".to_owned()))?;
    for part in parts {
        mac.update(part);
    }
    Ok(mac.finalize().into_bytes().into())
}

/// PBKDF2-HMAC-SHA256 with `iterations` rounds, `len` bytes.
pub fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32, len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    pbkdf2::pbkdf2_hmac::<Sha256>(password, salt, iterations, &mut out);
    out
}

/// AES-256 in CTR mode with a 128-bit big-endian counter. Encryption and decryption are the same
/// operation.
pub fn aes_ctr(key: &[u8; 32], iv: &[u8; 16], data: &mut [u8]) -> Result<()> {
    let mut cipher = ctr::Ctr128BE::<aes::Aes256>::new(key.into(), iv.into());
    cipher.apply_keystream(data);
    Ok(())
}

/// AES-256-GCM seal with no associated data.
pub fn gcm_seal(key: &[u8; 32], nonce: &[u8; 12], plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new(key.into());
    cipher
        .encrypt(&Nonce::from(*nonce), plaintext)
        .map_err(|_| Error::Internal("gcm seal failed".to_owned()))
}

/// AES-256-GCM open with no associated data. A bad tag is a protocol error.
pub fn gcm_open(key: &[u8; 32], nonce: &[u8; 12], ciphertext: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new(key.into());
    cipher
        .decrypt(&Nonce::from(*nonce), ciphertext)
        .map_err(|_| Error::Protocol("the key bundle does not authenticate".to_owned()))
}

/// The keys of one media type, expanded from its media key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaKeys {
    pub iv: [u8; 16],
    pub cipher_key: [u8; 32],
    pub mac_key: [u8; 32],
}

/// Expands a 32-byte media key for one media type (`WhatsApp Image Keys` and so on).
pub fn media_keys(media_key: &[u8], info: &str) -> Result<MediaKeys> {
    let expanded = hkdf(media_key, info.as_bytes(), 112)?;
    let (iv, rest) = expanded.split_at(16);
    let (cipher_key, rest) = rest.split_at(32);
    let (mac_key, _reference) = rest.split_at(32);
    Ok(MediaKeys {
        iv: fixed(iv)?,
        cipher_key: fixed(cipher_key)?,
        mac_key: fixed(mac_key)?,
    })
}

fn fixed<const N: usize>(bytes: &[u8]) -> Result<[u8; N]> {
    <[u8; N]>::try_from(bytes).map_err(|_| Error::Internal("a key has the wrong length".to_owned()))
}

/// The result of encrypting a media file for upload.
#[derive(Clone, Debug)]
pub struct EncryptedMedia {
    /// Ciphertext followed by the 10-byte MAC: the bytes to upload.
    pub data: Vec<u8>,
    pub file_sha256: [u8; 32],
    pub file_enc_sha256: [u8; 32],
}

/// Encrypts `plaintext` for `media_key` (a fresh random one for each upload).
pub fn encrypt_media(plaintext: &[u8], media_key: &[u8; 32], info: &str) -> Result<EncryptedMedia> {
    let keys = media_keys(media_key, info)?;
    let ciphertext = aes_cbc_encrypt(&keys.cipher_key, &keys.iv, plaintext)
        .map_err(|_| Error::Internal("media encryption failed".to_owned()))?;
    let mac = hmac256(&keys.mac_key, &[&keys.iv, &ciphertext])?;
    let mut data = ciphertext;
    data.extend_from_slice(&mac[..MEDIA_MAC_LENGTH]);
    Ok(EncryptedMedia {
        file_sha256: sha256(plaintext),
        file_enc_sha256: sha256(&data),
        data,
    })
}

/// Decrypts a downloaded media file and checks its MAC and both hashes.
pub fn decrypt_media(
    data: &[u8],
    media_key: &[u8],
    info: &str,
    file_sha256: &[u8],
    file_enc_sha256: &[u8],
) -> Result<Vec<u8>> {
    let split = data
        .len()
        .checked_sub(MEDIA_MAC_LENGTH)
        .filter(|len| *len > 0)
        .ok_or_else(|| Error::Protocol("the media file is too short".to_owned()))?;
    if sha256(data).as_slice() != file_enc_sha256 {
        return Err(Error::Protocol(
            "the encrypted media hash does not match".to_owned(),
        ));
    }
    let (ciphertext, mac) = data.split_at(split);
    let keys = media_keys(media_key, info)?;
    let expected = hmac256(&keys.mac_key, &[&keys.iv, ciphertext])?;
    if !constant_time_eq(&expected[..MEDIA_MAC_LENGTH], mac) {
        return Err(Error::Protocol("the media MAC does not match".to_owned()));
    }
    let plaintext = aes_cbc_decrypt(&keys.cipher_key, &keys.iv, ciphertext)
        .map_err(|_| Error::Protocol("the media file does not decrypt".to_owned()))?;
    if sha256(&plaintext).as_slice() != file_sha256 {
        return Err(Error::Protocol("the media hash does not match".to_owned()));
    }
    Ok(plaintext)
}

/// Compares two byte strings without leaking where they first differ.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

/// Pads a plaintext before Signal encryption: `n` copies of the byte `n`, with `n` random in 1..=15.
pub fn pad_message(plaintext: &[u8], rng: &mut impl RngCore) -> Vec<u8> {
    let mut byte = [0u8; 1];
    rng.fill_bytes(&mut byte);
    let mut n = byte[0] & 0x0f;
    if n == 0 {
        n = 0x0f;
    }
    let mut out = plaintext.to_vec();
    out.extend(std::iter::repeat_n(n, usize::from(n)));
    out
}

/// Removes the padding of [`pad_message`]. Version 3 messages are not padded.
pub fn unpad_message(plaintext: &[u8], version: u32) -> Result<Vec<u8>> {
    if version == 3 {
        return Ok(plaintext.to_vec());
    }
    let last = *plaintext
        .last()
        .ok_or_else(|| Error::Protocol("the plaintext is empty".to_owned()))?;
    let n = usize::from(last);
    if n == 0 || n > plaintext.len() {
        return Err(Error::Protocol(
            "the plaintext padding is invalid".to_owned(),
        ));
    }
    let (body, pad) = plaintext.split_at(plaintext.len() - n);
    if pad.iter().any(|byte| *byte != last) {
        return Err(Error::Protocol(
            "the plaintext padding is invalid".to_owned(),
        ));
    }
    Ok(body.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_answer() {
        assert_eq!(
            hex::encode(sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn hmac512_rfc4231_case_2() {
        let mac = hmac512(b"Jefe", &[b"what do ya want for nothing?"]).expect("hmac");
        assert_eq!(
            hex::encode(mac),
            "164b7a7bfcf819e2e395fbe73b56e0a387bd64222e831fd610270cd7ea2505549758bf75c05a994a6d034f65f8f0e6fdcaeab1a34d4a6b4b636e070a38bce737"
        );
    }

    #[test]
    fn pbkdf2_rfc6070_style_vector() {
        // PBKDF2-HMAC-SHA256, "password", "salt", 1 iteration, 32 bytes (RFC 7914 test vector).
        let out = pbkdf2_sha256(b"passwd", b"salt", 1, 64);
        assert_eq!(hex::encode(&out[..16]), "55ac046e56e3089fec1691c22544b605");
    }

    #[test]
    fn aes_ctr_is_its_own_inverse_and_matches_sp800_38a() {
        // NIST SP 800-38A F.5.5 CTR-AES256.Encrypt, block 1.
        let key: [u8; 32] =
            hex::decode("603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4")
                .expect("hex")
                .try_into()
                .expect("key");
        let iv: [u8; 16] = hex::decode("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff")
            .expect("hex")
            .try_into()
            .expect("iv");
        let mut block = hex::decode("6bc1bee22e409f96e93d7e117393172a").expect("hex");
        aes_ctr(&key, &iv, &mut block).expect("ctr");
        assert_eq!(hex::encode(&block), "601ec313775789a5b7a7f504bbf3d228");
        aes_ctr(&key, &iv, &mut block).expect("ctr");
        assert_eq!(hex::encode(&block), "6bc1bee22e409f96e93d7e117393172a");
    }

    #[test]
    fn gcm_round_trip_and_tamper_detection() {
        let key = [7u8; 32];
        let nonce = [9u8; 12];
        let sealed = gcm_seal(&key, &nonce, b"bundle").expect("seal");
        assert_eq!(gcm_open(&key, &nonce, &sealed).expect("open"), b"bundle");
        let mut bad = sealed.clone();
        bad[0] ^= 1;
        assert!(gcm_open(&key, &nonce, &bad).is_err());
    }

    #[test]
    fn media_round_trip_with_fixed_key() {
        let media_key = [0x42u8; 32];
        let file = b"a small image body".to_vec();
        let enc = encrypt_media(&file, &media_key, "WhatsApp Image Keys").expect("encrypt");
        let dec = decrypt_media(
            &enc.data,
            &media_key,
            "WhatsApp Image Keys",
            &enc.file_sha256,
            &enc.file_enc_sha256,
        )
        .expect("decrypt");
        assert_eq!(dec, file);
    }

    #[test]
    fn media_keys_use_the_info_string() {
        let key = [1u8; 32];
        let image = media_keys(&key, "WhatsApp Image Keys").expect("keys");
        let video = media_keys(&key, "WhatsApp Video Keys").expect("keys");
        assert_ne!(image, video);
    }

    #[test]
    fn media_with_a_flipped_byte_is_rejected() {
        let media_key = [3u8; 32];
        let enc = encrypt_media(b"data", &media_key, "WhatsApp Audio Keys").expect("encrypt");
        let mut tampered = enc.data.clone();
        tampered[0] ^= 1;
        // The encrypted hash is checked first, so tampering is caught before decryption.
        assert!(
            decrypt_media(
                &tampered,
                &media_key,
                "WhatsApp Audio Keys",
                &enc.file_sha256,
                &enc.file_enc_sha256
            )
            .is_err()
        );
    }

    #[test]
    fn padding_round_trips_and_rejects_bad_padding() {
        struct Fixed(u8);
        impl RngCore for Fixed {
            fn next_u32(&mut self) -> u32 {
                u32::from(self.0)
            }
            fn next_u64(&mut self) -> u64 {
                u64::from(self.0)
            }
            fn fill_bytes(&mut self, dest: &mut [u8]) {
                dest.fill(self.0);
            }
            fn try_fill_bytes(&mut self, dest: &mut [u8]) -> std::result::Result<(), rand::Error> {
                self.fill_bytes(dest);
                Ok(())
            }
        }
        let padded = pad_message(b"hi", &mut Fixed(3));
        assert_eq!(padded, [b'h', b'i', 3, 3, 3]);
        assert_eq!(unpad_message(&padded, 2).expect("unpad"), b"hi");
        assert_eq!(unpad_message(b"hi", 3).expect("v3"), b"hi");
        assert!(unpad_message(&[b'h', 2, 3], 2).is_err());
        assert!(unpad_message(&[], 2).is_err());
        assert!(unpad_message(&[9], 2).is_err());
    }

    #[test]
    fn constant_time_eq_checks_length() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"ab", b"abc"));
    }
}
