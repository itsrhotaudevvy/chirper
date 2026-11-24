//! Field encryption of the password login: the page carries an RSA public key and a salt, and
//! fields it marks as encrypted are sent as `RSA-OAEP(salt || value)`, token-encoded. The salt's
//! server time is advanced by the age of the page, as the web client does.
//!
//! Only the RSA-OAEP primitive of the `rsa` crate is used. Reference: `linkedingo/login_sdui.go`.

use std::time::Instant;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use rsa::pkcs8::DecodePublicKey;
use rsa::{Oaep, RsaPublicKey};
use serde_json::Value;
use sha1::Sha1;

/// The key and salt of one bootstrap page.
pub(super) struct Encryption {
    public_key: String,
    salt: String,
    token_ttl_ms: f64,
    observed: Instant,
}

/// The modulus length the encryption accepts: 2048 bits.
const MODULUS_BYTES: usize = 256;
/// The salt is a big-endian server time in milliseconds.
const SALT_BYTES: usize = 8;
/// The token prefix the web client uses for an RSA-OAEP field.
const TOKEN_PREFIX: [u8; 2] = [1, 1];

/// Finds the encryption metadata anywhere in a hydration value.
pub(super) fn find_encryption(value: &Value, depth: usize) -> Option<Encryption> {
    if depth > 100 {
        return None;
    }
    match value {
        Value::Object(map) => {
            if let (Some(key), Some(salt)) = (
                map.get("rsaPublicKey").and_then(Value::as_str),
                map.get("encryptionSalt").and_then(Value::as_str),
            ) {
                let token_ttl_ms = map.get("tokenTtlMs").and_then(Value::as_f64).unwrap_or(0.0);
                return Some(Encryption {
                    public_key: key.to_owned(),
                    salt: salt.to_owned(),
                    token_ttl_ms,
                    observed: Instant::now(),
                });
            }
            map.values()
                .find_map(|child| find_encryption(child, depth + 1))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|child| find_encryption(child, depth + 1)),
        _ => None,
    }
}

impl Encryption {
    /// The token for `value`. `None` when the key or salt is not one this encryption accepts;
    /// the caller then refuses to send the field rather than sending it in the clear.
    pub(super) fn encrypt(&self, value: &str) -> Option<String> {
        let der = STANDARD.decode(&self.public_key).ok()?;
        let key = RsaPublicKey::from_public_key_der(&der).ok()?;
        if rsa::traits::PublicKeyParts::size(&key) != MODULUS_BYTES {
            return None;
        }
        let mut salt = STANDARD.decode(&self.salt).ok()?;
        if salt.len() != SALT_BYTES {
            return None;
        }
        self.advance_salt(&mut salt);
        let mut plaintext = salt;
        plaintext.extend_from_slice(value.as_bytes());
        let mut rng = rand::thread_rng();
        let ciphertext = key
            .encrypt(&mut rng, Oaep::new::<Sha1>(), &plaintext)
            .ok()?;
        let mut token = TOKEN_PREFIX.to_vec();
        token.extend_from_slice(&ciphertext);
        Some(URL_SAFE_NO_PAD.encode(token))
    }

    /// Moves the salt's server time forward by the age of the page, when the page has a TTL.
    fn advance_salt(&self, salt: &mut [u8]) {
        if self.token_ttl_ms <= 0.0 {
            return;
        }
        let elapsed = u64::try_from(self.observed.elapsed().as_millis()).unwrap_or(u64::MAX);
        let Ok(bytes) = <[u8; SALT_BYTES]>::try_from(&*salt) else {
            return;
        };
        let server_time = u64::from_be_bytes(bytes);
        if server_time != 0 && elapsed > 0 {
            let advanced = server_time.saturating_add(elapsed);
            salt.copy_from_slice(&advanced.to_be_bytes());
        }
    }
}
