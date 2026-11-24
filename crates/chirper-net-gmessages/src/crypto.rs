//! The cryptography of the Google Messages protocol, from `libgm/crypto`:
//!
//! * [`RequestCrypto`]: AES-256-CTR with a random 16-byte IV, authenticated with HMAC-SHA256 over
//!   `ciphertext || iv`. Every request body and most response payloads of the phone link use it.
//! * [`GcmChunks`]: the chunked AES-256-GCM container of media files (`aesgcm.go`).
//! * P-256 helpers for the refresh key (ECDSA), the Gaia pairing (ECDH) and the UKEY2 key schedule.
//!
//! Only RustCrypto and `ring` primitives are used; nothing here implements a cipher or a MAC.

use aes::Aes256;
use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use ctr::cipher::{KeyIvInit, StreamCipher};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use base64::Engine;
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use chirper_bridge::{Error, Result};
use serde::Deserialize;

/// The big-endian 128-bit counter mode the Go `cipher.NewCTR` uses.
type Aes256Ctr = ctr::Ctr128BE<Aes256>;
type HmacSha256 = Hmac<Sha256>;

/// Length of the AES and HMAC keys of [`RequestCrypto`].
pub const KEY_LEN: usize = 32;
/// Length of the CTR IV that precedes the ciphertext.
const IV_LEN: usize = 16;
/// Length of the HMAC tag that ends every [`RequestCrypto`] envelope.
const MAC_LEN: usize = 32;

/// `n` random bytes from the OS source.
pub fn random_bytes(n: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf).map_err(|err| Error::Internal(format!("no random source: {err}")))?;
    Ok(buf)
}

/// A random RFC 4122 version 4 UUID in its lowercase hyphenated form, as the Go client makes them.
pub fn uuid_v4() -> Result<String> {
    let mut b = random_bytes(16)?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex = hex::encode(&b);
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

/// A 32-byte key that is wiped when dropped.
pub type Key32 = Zeroizing<[u8; KEY_LEN]>;

/// The AES and HMAC keys of one paired session. The phone and the browser share them.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct RequestCrypto {
    #[serde(with = "key_b64")]
    aes_key: Key32,
    #[serde(with = "key_b64")]
    hmac_key: Key32,
}

impl std::fmt::Debug for RequestCrypto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RequestCrypto([redacted])")
    }
}

impl RequestCrypto {
    /// Fresh random keys, as `NewAESCTRHelper` makes them.
    pub fn random() -> Result<Self> {
        Ok(Self {
            aes_key: random_key()?,
            hmac_key: random_key()?,
        })
    }

    /// Keys from known material (the UKEY2 key schedule).
    pub fn from_keys(aes_key: Key32, hmac_key: Key32) -> Self {
        Self { aes_key, hmac_key }
    }

    /// The AES and HMAC keys, in the order the QR code carries them (AES first).
    pub fn key_pair_bytes(&self) -> (&[u8], &[u8]) {
        (self.aes_key.as_slice(), self.hmac_key.as_slice())
    }

    /// `iv || ciphertext || mac`, the envelope the phone expects.
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let iv = random_bytes(IV_LEN)?;
        let mut out = Vec::with_capacity(plaintext.len() + IV_LEN + MAC_LEN);
        out.extend_from_slice(plaintext);
        let mut cipher = aes_ctr(self.aes_key.as_slice(), &iv)?;
        cipher.apply_keystream(&mut out);
        out.extend_from_slice(&iv);
        let tag = self.mac(&out)?;
        out.extend_from_slice(&tag);
        Ok(out)
    }

    /// Check the tag and decrypt an envelope made by [`RequestCrypto::encrypt`] or the phone.
    pub fn decrypt(&self, envelope: &[u8]) -> Result<Vec<u8>> {
        if envelope.len() < IV_LEN + MAC_LEN {
            return Err(Error::Protocol("encrypted payload is too short".to_owned()));
        }
        let (body, tag) = envelope.split_at(envelope.len() - MAC_LEN);
        let mut mac = self.mac_state()?;
        mac.update(body);
        mac.verify_slice(tag)
            .map_err(|_| Error::Protocol("encrypted payload failed its HMAC check".to_owned()))?;
        let (cipher_text, iv) = body.split_at(body.len() - IV_LEN);
        let mut out = cipher_text.to_vec();
        let mut cipher = aes_ctr(self.aes_key.as_slice(), iv)?;
        cipher.apply_keystream(&mut out);
        Ok(out)
    }

    fn mac(&self, data: &[u8]) -> Result<[u8; MAC_LEN]> {
        let mut mac = self.mac_state()?;
        mac.update(data);
        Ok(mac.finalize().into_bytes().into())
    }

    fn mac_state(&self) -> Result<HmacSha256> {
        HmacSha256::new_from_slice(self.hmac_key.as_slice())
            .map_err(|_| Error::Internal("the HMAC key has an invalid length".to_owned()))
    }
}

/// The chunked AES-256-GCM container of media and key-wrapped payloads (`aesgcm.go`).
///
/// Format: a two-byte header `0x00, log2(chunk size)`, then chunks of `chunk` bytes, each
/// `nonce(12) || ciphertext || tag(16)`. The associated data is one flag byte (1 for the last chunk)
/// followed by the big-endian chunk index.
pub struct GcmChunks {
    cipher: Aes256Gcm,
}

/// Raw size of one encrypted chunk the sender uses (`1 << 15`).
pub const GCM_OUT_CHUNK: usize = 1 << 15;
/// Largest raw chunk the reader accepts (`1 << 22`).
const GCM_MAX_CHUNK: usize = 1 << 22;
const GCM_NONCE: usize = 12;
const GCM_TAG: usize = 16;

impl GcmChunks {
    /// A container for a 32-byte key.
    pub fn new(key: &[u8]) -> Result<Self> {
        let cipher = Aes256Gcm::new_from_slice(key)
            .map_err(|_| Error::Protocol("the media key has an unsupported length".to_owned()))?;
        Ok(Self { cipher })
    }

    /// Encrypt a whole buffer in the sender's chunking.
    pub fn encrypt(&self, data: &[u8]) -> Result<Vec<u8>> {
        let chunk = GCM_OUT_CHUNK - GCM_NONCE - GCM_TAG;
        let mut out = vec![0u8, GCM_OUT_CHUNK.trailing_zeros() as u8];
        if data.is_empty() {
            return Ok(out);
        }
        let mut index: u32 = 0;
        let mut offset = 0;
        loop {
            let end = (offset + chunk).min(data.len());
            let last = offset + chunk >= data.len();
            let nonce = random_bytes(GCM_NONCE)?;
            let aad = chunk_aad(index, last);
            let sealed = self
                .cipher
                .encrypt(
                    &nonce_of(&nonce)?,
                    Payload {
                        msg: &data[offset..end],
                        aad: &aad,
                    },
                )
                .map_err(|_| Error::Internal("AES-GCM sealing failed".to_owned()))?;
            out.extend_from_slice(&nonce);
            out.extend_from_slice(&sealed);
            index = index.wrapping_add(1);
            offset = end;
            if last {
                break;
            }
        }
        Ok(out)
    }

    /// Decrypt a whole container. Every chunk is authenticated; a truncated or reordered stream fails.
    pub fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>> {
        let header = data
            .get(..2)
            .ok_or_else(|| Error::Protocol("media container is too short".to_owned()))?;
        if header[0] != 0 {
            return Err(Error::Protocol(
                "media container has an unknown header".to_owned(),
            ));
        }
        let chunk = 1usize
            .checked_shl(u32::from(header[1]))
            .filter(|size| *size > GCM_NONCE + GCM_TAG && *size <= GCM_MAX_CHUNK)
            .ok_or_else(|| Error::Protocol("media container has a bad chunk size".to_owned()))?;
        let body = &data[2..];
        let mut out = Vec::with_capacity(body.len());
        let mut index: u32 = 0;
        let mut offset = 0;
        while offset < body.len() {
            let end = (offset + chunk).min(body.len());
            let last = offset + chunk >= body.len();
            let sealed = body
                .get(offset..end)
                .ok_or_else(|| Error::Protocol("media container is truncated".to_owned()))?;
            let (nonce, ciphertext) = sealed.split_at(GCM_NONCE);
            let aad = chunk_aad(index, last);
            let plain = self
                .cipher
                .decrypt(
                    &nonce_of(nonce)?,
                    Payload {
                        msg: ciphertext,
                        aad: &aad,
                    },
                )
                .map_err(|_| Error::Protocol("media chunk failed authentication".to_owned()))?;
            out.extend_from_slice(&plain);
            index = index.wrapping_add(1);
            offset = end;
        }
        Ok(out)
    }
}

fn aes_ctr(key: &[u8], iv: &[u8]) -> Result<Aes256Ctr> {
    Aes256Ctr::new_from_slices(key, iv)
        .map_err(|_| Error::Internal("the AES key or IV has an invalid length".to_owned()))
}

fn nonce_of(bytes: &[u8]) -> Result<Nonce<aes_gcm::aead::consts::U12>> {
    Nonce::<aes_gcm::aead::consts::U12>::try_from(bytes)
        .map_err(|_| Error::Internal("the AES-GCM nonce has an invalid length".to_owned()))
}

fn chunk_aad(index: u32, last: bool) -> [u8; 5] {
    let mut aad = [0u8; 5];
    aad[0] = u8::from(last);
    aad[1..5].copy_from_slice(&index.to_be_bytes());
    aad
}

/// HKDF-SHA256 with a 32-byte output, the Go `doHKDF` helper.
pub fn hkdf32(secret: &[u8], salt: &[u8], info: &[u8]) -> Result<Key32> {
    let hk = Hkdf::<Sha256>::new(Some(salt), secret);
    let mut out = Zeroizing::new([0u8; KEY_LEN]);
    hk.expand(info, out.as_mut_slice())
        .map_err(|_| Error::Internal("HKDF output length is invalid".to_owned()))?;
    Ok(out)
}

/// SHA-256 of `data`.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

fn random_key() -> Result<Key32> {
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    let bytes = random_bytes(KEY_LEN)?;
    key.copy_from_slice(&bytes);
    Ok(key)
}

/// A P-256 key pair kept as PKCS#8 (the form `ring` signs with). Its `Debug` output is redacted.
#[derive(Clone)]
pub struct RefreshKey {
    pkcs8: Zeroizing<Vec<u8>>,
}

impl std::fmt::Debug for RefreshKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RefreshKey([redacted])")
    }
}

impl RefreshKey {
    /// A new random P-256 key.
    pub fn generate() -> Result<Self> {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng)
            .map_err(|_| Error::Internal("could not generate a P-256 key".to_owned()))?;
        Ok(Self {
            pkcs8: Zeroizing::new(pkcs8.as_ref().to_vec()),
        })
    }

    /// Restore a key from its stored PKCS#8 bytes.
    pub fn from_pkcs8(bytes: &[u8]) -> Result<Self> {
        let rng = SystemRandom::new();
        EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, bytes, &rng)
            .map_err(|_| Error::Auth("the stored refresh key is invalid".to_owned()))?;
        Ok(Self {
            pkcs8: Zeroizing::new(bytes.to_vec()),
        })
    }

    /// The PKCS#8 bytes to store.
    pub fn pkcs8(&self) -> &[u8] {
        &self.pkcs8
    }

    fn pair(&self) -> Result<EcdsaKeyPair> {
        EcdsaKeyPair::from_pkcs8(
            &ECDSA_P256_SHA256_ASN1_SIGNING,
            &self.pkcs8,
            &SystemRandom::new(),
        )
        .map_err(|_| Error::Auth("the stored refresh key is invalid".to_owned()))
    }

    /// The uncompressed public point (`0x04 || X || Y`).
    pub fn public_point(&self) -> Result<Vec<u8>> {
        Ok(self.pair()?.public_key().as_ref().to_vec())
    }

    /// The DER `SubjectPublicKeyInfo` of the public key, as `x509.MarshalPKIXPublicKey` writes it.
    pub fn spki_der(&self) -> Result<Vec<u8>> {
        let point = self.public_point()?;
        let mut der = vec![
            0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06,
            0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
        ];
        der.extend_from_slice(&point);
        Ok(der)
    }

    /// ECDSA over `message` (hashed with SHA-256 first), DER encoded, as `ecdsa.SignASN1` does
    /// over a digest.
    pub fn sign(&self, message: &[u8]) -> Result<Vec<u8>> {
        let rng = SystemRandom::new();
        let sig = self
            .pair()?
            .sign(&rng, message)
            .map_err(|_| Error::Internal("could not sign with the refresh key".to_owned()))?;
        Ok(sig.as_ref().to_vec())
    }
}

impl serde::Serialize for RefreshKey {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(self.pkcs8.as_slice()))
    }
}

impl<'de> serde::Deserialize<'de> for RefreshKey {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(text)
            .map_err(serde::de::Error::custom)?;
        RefreshKey::from_pkcs8(&bytes).map_err(serde::de::Error::custom)
    }
}

/// An ephemeral P-256 key for the UKEY2 handshake.
pub struct EphemeralP256 {
    key: ring::agreement::EphemeralPrivateKey,
    public: Vec<u8>,
}

impl EphemeralP256 {
    /// A fresh key.
    pub fn generate() -> Result<Self> {
        let rng = SystemRandom::new();
        let key = ring::agreement::EphemeralPrivateKey::generate(&ring::agreement::ECDH_P256, &rng)
            .map_err(|_| Error::Internal("could not generate an ECDH key".to_owned()))?;
        let public = key
            .compute_public_key()
            .map_err(|_| Error::Internal("could not derive an ECDH public key".to_owned()))?
            .as_ref()
            .to_vec();
        Ok(Self { key, public })
    }

    /// The uncompressed public point.
    pub fn public_point(&self) -> &[u8] {
        &self.public
    }

    /// The x coordinate of the shared point with `peer` (an uncompressed point), as Go's `ECDH`
    /// returns it.
    pub fn agree(self, peer: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        let peer_key = ring::agreement::UnparsedPublicKey::new(&ring::agreement::ECDH_P256, peer);
        ring::agreement::agree_ephemeral(self.key, &peer_key, |secret| {
            Zeroizing::new(secret.to_vec())
        })
        .map_err(|_| Error::Protocol("the server's ECDH key is invalid".to_owned()))
    }
}

/// Verify an ECDSA P-256 signature over `message` (SHA-256 inside). Used by tests of the signer.
#[cfg(test)]
pub fn verify_p256(public_point: &[u8], message: &[u8], signature: &[u8]) -> bool {
    use ring::signature::ECDSA_P256_SHA256_ASN1;
    ring::signature::UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, public_point)
        .verify(message, signature)
        .is_ok()
}

mod key_b64 {
    use base64::Engine;
    use zeroize::Zeroize;
    use serde::{Deserialize, Deserializer, Serializer};

    use super::Key32;

    pub fn serialize<S: Serializer>(key: &Key32, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(key.as_slice()))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Key32, D::Error> {
        let text = String::deserialize(d)?;
        let mut bytes = base64::engine::general_purpose::STANDARD
            .decode(text)
            .map_err(serde::de::Error::custom)?;
        let mut key = Key32::default();
        if bytes.len() != super::KEY_LEN {
            bytes.zeroize();
            return Err(serde::de::Error::custom("a key has the wrong length"));
        }
        key.copy_from_slice(&bytes);
        bytes.zeroize();
        Ok(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Envelope made by the Go client's `AESCTRHelper.Encrypt` (libgm/crypto/aesctr.go) with
    /// AES key `000102..1f`, HMAC key `ffeeddcc..1100`, plaintext `Hello, Google Messages!`.
    const GO_CTR_ENVELOPE: &str = "b71c9cbf28de303f9e78ee3b82b864d6cf70ffd1ec616551d89139a8ffa51827077e6c2503dd8dc813713f0023775c38cc15453229b531ace089224a51c30db1968f8a90d57822";
    /// Envelope made by the Go client's `AESGCMHelper.EncryptData` with the key `00..1f`, plaintext `Hello`.
    const GO_GCM_ENVELOPE: &str = "000f54a9d08a3e24ad2a5d3ca1f53e6b15498326f45c7868ea3c0b887619079a2fb8ef";

    fn ascending_key() -> Key32 {
        let mut key = Key32::default();
        for (i, byte) in key.iter_mut().enumerate() {
            *byte = i as u8;
        }
        key
    }

    fn descending_key() -> Key32 {
        let mut key = Key32::default();
        let bytes = hex::decode("ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100")
            .expect("hex");
        key.copy_from_slice(&bytes);
        key
    }

    fn go_ctr() -> RequestCrypto {
        RequestCrypto::from_keys(ascending_key(), descending_key())
    }

    #[test]
    fn decrypts_an_envelope_made_by_the_go_client() {
        let envelope = hex::decode(GO_CTR_ENVELOPE).expect("hex");
        let plain = go_ctr().decrypt(&envelope).expect("decrypts");
        assert_eq!(plain, b"Hello, Google Messages!");
    }

    #[test]
    fn round_trips_and_never_repeats_an_iv() {
        let crypto = go_ctr();
        let a = crypto.encrypt(b"same text").expect("encrypts");
        let b = crypto.encrypt(b"same text").expect("encrypts");
        assert_ne!(a, b, "a fresh IV per message");
        assert_eq!(crypto.decrypt(&a).expect("decrypts"), b"same text");
        assert_eq!(crypto.decrypt(&b).expect("decrypts"), b"same text");
    }

    #[test]
    fn a_flipped_bit_fails_the_mac() {
        let crypto = go_ctr();
        let mut envelope = crypto.encrypt(b"payload").expect("encrypts");
        envelope[0] ^= 1;
        assert!(matches!(crypto.decrypt(&envelope), Err(Error::Protocol(_))));
    }

    #[test]
    fn a_short_envelope_is_rejected_before_any_work() {
        assert!(matches!(go_ctr().decrypt(&[0u8; 47]), Err(Error::Protocol(_))));
    }

    #[test]
    fn decrypts_a_gcm_container_made_by_the_go_client() {
        let envelope = hex::decode(GO_GCM_ENVELOPE).expect("hex");
        let gcm = GcmChunks::new(ascending_key().as_slice()).expect("key");
        assert_eq!(gcm.decrypt(&envelope).expect("decrypts"), b"Hello");
    }

    #[test]
    fn gcm_round_trips_across_several_chunks() {
        let gcm = GcmChunks::new(ascending_key().as_slice()).expect("key");
        let data: Vec<u8> = (0..(GCM_OUT_CHUNK * 2 + 123)).map(|i| (i % 251) as u8).collect();
        let sealed = gcm.encrypt(&data).expect("encrypts");
        assert_eq!(sealed[1], 15, "the header records log2 of the chunk size");
        assert_eq!(gcm.decrypt(&sealed).expect("decrypts"), data);
    }

    #[test]
    fn gcm_refuses_a_truncated_container() {
        let gcm = GcmChunks::new(ascending_key().as_slice()).expect("key");
        let data = vec![7u8; GCM_OUT_CHUNK * 2];
        let mut sealed = gcm.encrypt(&data).expect("encrypts");
        sealed.truncate(sealed.len() - 40);
        assert!(gcm.decrypt(&sealed).is_err());
    }

    #[test]
    fn hkdf_matches_rfc_5869_case_1() {
        let ikm = [0x0bu8; 22];
        let salt: Vec<u8> = (0x00..=0x0c).collect();
        let info: Vec<u8> = (0xf0..=0xf9).collect();
        let okm = hkdf32(&ikm, &salt, &info).expect("derives");
        assert_eq!(
            hex::encode(okm.as_slice()),
            "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf"
        );
    }

    #[test]
    fn refresh_key_signs_and_exports_a_p256_spki() {
        let key = RefreshKey::generate().expect("generates");
        let spki = key.spki_der().expect("spki");
        assert_eq!(spki.len(), 91);
        assert_eq!(&spki[..4], &[0x30, 0x59, 0x30, 0x13]);
        let point = key.public_point().expect("point");
        assert_eq!(point[0], 4);
        let sig = key.sign(b"request:1").expect("signs");
        assert!(verify_p256(&point, b"request:1", &sig));
        assert!(!verify_p256(&point, b"request:2", &sig));
        let restored = RefreshKey::from_pkcs8(key.pkcs8()).expect("restores");
        assert_eq!(restored.public_point().expect("point"), point);
    }

    #[test]
    fn ecdh_is_symmetric() {
        let a = EphemeralP256::generate().expect("a");
        let b = EphemeralP256::generate().expect("b");
        let a_pub = a.public_point().to_vec();
        let b_pub = b.public_point().to_vec();
        let ab = a.agree(&b_pub).expect("a agrees");
        let ba = b.agree(&a_pub).expect("b agrees");
        assert_eq!(ab.as_slice(), ba.as_slice());
        assert_eq!(ab.len(), 32);
    }

    #[test]
    fn uuids_are_version_4_and_unique() {
        let a = uuid_v4().expect("uuid");
        let b = uuid_v4().expect("uuid");
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(a.as_bytes()[14], b'4');
        assert!(matches!(a.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
    }

    #[test]
    fn request_crypto_serialises_its_keys_as_base64_and_hides_them_in_debug() {
        let crypto = go_ctr();
        let json = serde_json::to_string(&crypto).expect("serialises");
        let back: RequestCrypto = serde_json::from_str(&json).expect("parses");
        let envelope = crypto.encrypt(b"x").expect("encrypts");
        assert_eq!(back.decrypt(&envelope).expect("decrypts"), b"x");
        assert_eq!(format!("{crypto:?}"), "RequestCrypto([redacted])");
    }
}
