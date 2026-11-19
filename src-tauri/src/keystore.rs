//! The 32-byte database key on the desktop: kept in the OS credential store, never in a file.
//!
//! Android and iOS get their key from the `chirper-native` plugin (Keystore / Keychain); this module
//! only serves macOS, Windows and Linux.

use zeroize::Zeroizing;

pub const KEY_SERVICE: &str = "app.chirper";
pub const KEY_USER: &str = "db-key";
pub const KEY_LEN: usize = 32;

fn random_key() -> Result<Zeroizing<[u8; KEY_LEN]>, String> {
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    getrandom::fill(&mut key[..]).map_err(|e| format!("random source failed: {e}"))?;
    Ok(key)
}

fn encode_hex(key: &[u8; KEY_LEN]) -> Zeroizing<String> {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(KEY_LEN * 2);
    for byte in key {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    Zeroizing::new(out)
}

fn decode_hex(text: &str) -> Result<Zeroizing<[u8; KEY_LEN]>, String> {
    let malformed = || "stored database key is malformed".to_owned();
    if text.len() != KEY_LEN * 2 {
        return Err(malformed());
    }
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    let bytes = text.as_bytes();
    for (i, pair) in bytes.chunks_exact(2).enumerate() {
        let hi = hex_value(pair[0]).ok_or_else(malformed)?;
        let lo = hex_value(pair[1]).ok_or_else(malformed)?;
        key[i] = (hi << 4) | lo;
    }
    Ok(key)
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// The key in the platform credential store: Keychain (macOS), Credential Manager (Windows) or the
/// Secret Service (Linux), under service `app.chirper` and user `db-key`.
pub fn load_or_create() -> Result<Zeroizing<[u8; KEY_LEN]>, String> {
    let entry =
        keyring::Entry::new(KEY_SERVICE, KEY_USER).map_err(|e| format!("credential store: {e}"))?;
    match entry.get_password() {
        Ok(stored) => decode_hex(&Zeroizing::new(stored)),
        // Overwriting an unreadable key would make the database unrecoverable, so only a missing
        // entry leads to creation.
        Err(keyring::Error::NoEntry) => {
            let key = random_key()?;
            entry
                .set_password(&encode_hex(&key))
                .map_err(|e| format!("credential store: {e}"))?;
            Ok(key)
        }
        Err(e) => Err(format!("credential store: {e}")),
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
        assert!(decode_hex("abc").is_err());
        assert!(decode_hex(&"g".repeat(64)).is_err());
    }
}
