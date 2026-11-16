//! The on-disk format of the Android wrapped database key.
//!
//! Layout: one version byte, the 12-byte AES-GCM IV, then the ciphertext of the 32-byte key with its
//! 16-byte tag appended. The wrapping key lives in the Android Keystore and never leaves it.

use std::fmt;

pub const VERSION: u8 = 1;
pub const IV_LEN: usize = 12;
pub const KEY_LEN: usize = 32;
const TAG_LEN: usize = 16;
const WRAPPED_LEN: usize = KEY_LEN + TAG_LEN;
const HEADER_LEN: usize = 1 + IV_LEN;

#[derive(Debug, PartialEq, Eq)]
pub struct FormatError(&'static str);

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for FormatError {}

/// Builds the file contents for a wrapped key. Rejects anything that is not a 12-byte IV and a
/// wrapped 32-byte key, so a broken wrap never reaches the disk.
pub fn encode(iv: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, FormatError> {
    if iv.len() != IV_LEN {
        return Err(FormatError("wrapped key IV has the wrong length"));
    }
    if ciphertext.len() != WRAPPED_LEN {
        return Err(FormatError("wrapped key has the wrong length"));
    }
    let mut out = Vec::with_capacity(HEADER_LEN + ciphertext.len());
    out.push(VERSION);
    out.extend_from_slice(iv);
    out.extend_from_slice(ciphertext);
    Ok(out)
}

/// Splits file contents into the IV and the ciphertext. Unknown versions are refused.
pub fn decode(bytes: &[u8]) -> Result<(&[u8], &[u8]), FormatError> {
    let (&version, rest) = bytes
        .split_first()
        .ok_or(FormatError("wrapped key file is empty"))?;
    if version != VERSION {
        return Err(FormatError("wrapped key file has an unknown version"));
    }
    if rest.len() != IV_LEN + WRAPPED_LEN {
        return Err(FormatError("wrapped key file has the wrong length"));
    }
    Ok(rest.split_at(IV_LEN))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_then_decode_round_trips() {
        let iv = [7u8; IV_LEN];
        let ct = [9u8; WRAPPED_LEN];
        let file = encode(&iv, &ct).unwrap();
        assert_eq!(file[0], VERSION);
        assert_eq!(file.len(), HEADER_LEN + WRAPPED_LEN);
        let (got_iv, got_ct) = decode(&file).unwrap();
        assert_eq!(got_iv, iv);
        assert_eq!(got_ct, ct);
    }

    #[test]
    fn encode_rejects_bad_lengths() {
        assert!(encode(&[0u8; 11], &[0u8; WRAPPED_LEN]).is_err());
        assert!(encode(&[0u8; IV_LEN], &[0u8; WRAPPED_LEN - 1]).is_err());
        assert!(encode(&[0u8; IV_LEN], &[0u8; WRAPPED_LEN + 1]).is_err());
    }

    #[test]
    fn decode_rejects_truncated_empty_and_unknown_versions() {
        let good = encode(&[1u8; IV_LEN], &[2u8; WRAPPED_LEN]).unwrap();
        assert!(decode(&[]).is_err());
        assert!(decode(&good[..good.len() - 1]).is_err());
        let mut versioned = good.clone();
        versioned[0] = VERSION + 1;
        assert!(decode(&versioned).is_err());
        let mut extended = good;
        extended.push(0);
        assert!(decode(&extended).is_err());
    }
}
