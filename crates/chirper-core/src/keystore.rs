//! The 32-byte database key, kept in the OS credential store and never in files.

use std::sync::Mutex;

use zeroize::Zeroizing;

use crate::{CoreError, Result};

pub const KEY_SERVICE: &str = "app.chirper";
pub const KEY_USER: &str = "db-key";
const KEY_LEN: usize = 32;

pub trait KeyStore: Send + Sync {
    /// Returns the stored key, creating and storing a random one on first use.
    fn load_or_create(&self) -> Result<Zeroizing<[u8; KEY_LEN]>>;
}

fn random_key() -> Result<Zeroizing<[u8; KEY_LEN]>> {
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    getrandom::fill(&mut key[..])
        .map_err(|e| CoreError::KeyStore(format!("random source failed: {e}")))?;
    Ok(key)
}

#[cfg(any(test, not(target_os = "android")))]
fn encode_hex(key: &[u8; KEY_LEN]) -> Zeroizing<String> {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(KEY_LEN * 2);
    for byte in key {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    Zeroizing::new(out)
}

#[cfg(any(test, not(target_os = "android")))]
fn decode_hex(text: &str) -> Result<Zeroizing<[u8; KEY_LEN]>> {
    let malformed = || CoreError::KeyStore("stored database key is malformed".to_owned());
    if text.len() != KEY_LEN * 2 {
        return Err(malformed());
    }
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    for (i, [hi, lo]) in text.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let hi = hex_value(*hi).ok_or_else(malformed)?;
        let lo = hex_value(*lo).ok_or_else(malformed)?;
        key[i] = (hi << 4) | lo;
    }
    Ok(key)
}

#[cfg(any(test, not(target_os = "android")))]
fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(not(target_os = "android"))]
pub use os::OsKeyStore;

#[cfg(not(target_os = "android"))]
mod os {
    use super::*;

    /// The key in the platform credential store: Keychain (macOS, iOS), Credential Manager
    /// (Windows) or the Secret Service (Linux), under service `app.chirper` and user `db-key`.
    #[derive(Debug, Default, Clone, Copy)]
    pub struct OsKeyStore;

    fn keyring_error(e: keyring::Error) -> CoreError {
        CoreError::KeyStore(format!("credential store: {e}"))
    }

    impl KeyStore for OsKeyStore {
        fn load_or_create(&self) -> Result<Zeroizing<[u8; KEY_LEN]>> {
            let entry = keyring::Entry::new(KEY_SERVICE, KEY_USER).map_err(keyring_error)?;
            match entry.get_password() {
                Ok(stored) => decode_hex(&Zeroizing::new(stored)),
                // Overwriting an unreadable key would make the database unrecoverable, so only a
                // missing entry leads to creation.
                Err(keyring::Error::NoEntry) => {
                    let key = random_key()?;
                    entry
                        .set_password(&encode_hex(&key))
                        .map_err(keyring_error)?;
                    Ok(key)
                }
                Err(e) => Err(keyring_error(e)),
            }
        }
    }
}

/// Android keeps the key in the Android Keystore, which the `chirper-native` plugin provides.
#[cfg(target_os = "android")]
#[derive(Debug, Default, Clone, Copy)]
pub struct OsKeyStore;

#[cfg(target_os = "android")]
impl KeyStore for OsKeyStore {
    fn load_or_create(&self) -> Result<Zeroizing<[u8; KEY_LEN]>> {
        Err(CoreError::KeyStore(
            "android keystore is provided by the chirper-native plugin".to_owned(),
        ))
    }
}

/// A key kept in memory only. It is used by tests and by callers that manage persistence
/// themselves. Nothing is written to disk.
#[derive(Debug, Default)]
pub struct MemoryKeyStore {
    key: Mutex<Option<Zeroizing<[u8; KEY_LEN]>>>,
}

impl MemoryKeyStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl KeyStore for MemoryKeyStore {
    fn load_or_create(&self) -> Result<Zeroizing<[u8; KEY_LEN]>> {
        let mut slot = self
            .key
            .lock()
            .map_err(|_| CoreError::KeyStore("memory key store poisoned".to_owned()))?;
        if let Some(key) = slot.as_ref() {
            return Ok(Zeroizing::new(**key));
        }
        let key = random_key()?;
        let copy = Zeroizing::new(*key);
        *slot = Some(key);
        Ok(copy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip_and_rejections() {
        let key = random_key().unwrap();
        let text = encode_hex(&key);
        assert_eq!(text.len(), 64);
        assert_eq!(*decode_hex(&text).unwrap(), *key);
        assert_eq!(*decode_hex(&text.to_uppercase()).unwrap(), *key);
        assert!(decode_hex("abc").is_err());
        assert!(decode_hex(&"g".repeat(64)).is_err());
    }

    #[test]
    fn memory_store_returns_the_same_key() {
        let store = MemoryKeyStore::new();
        let first = store.load_or_create().unwrap();
        let second = store.load_or_create().unwrap();
        assert_eq!(*first, *second);
        assert_ne!(*first, [0u8; KEY_LEN]);
        let other = MemoryKeyStore::new().load_or_create().unwrap();
        assert_ne!(*first, *other);
    }
}
