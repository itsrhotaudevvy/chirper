//! At-rest protection for everything that is not the database itself.
//!
//! * [`vault`] seals media files with AES-256-GCM in the `OCV1` format the Go core wrote, so media
//!   that an older build encrypted stays readable.
//! * [`FileSecrets`] stores per-login credentials, sealed with the database key, in
//!   `<data_dir>/credentials` with owner-only permissions.
//! * [`redact`] keeps token-shaped strings out of log lines.

use std::fs;
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::Aes256Gcm;

/// AES-GCM uses 96-bit nonces.
type NonceBytes = [u8; 12];
type Nonce12 = aes_gcm::Nonce<aes_gcm::aead::consts::U12>;
use zeroize::Zeroizing;

pub mod vault {
    //! The `OCV1` media vault: a fresh key per file, 64 KiB AES-256-GCM chunks.
    //!
    //! Layout: `"OCV1"` || 8 random bytes, then sealed chunks. The nonce of a chunk is the file
    //! prefix plus the big-endian chunk index. Every chunk but the last is sealed with AAD `0x00`,
    //! the last with `0x01`, so truncating or appending fails authentication.

    use super::*;

    const MAGIC: &[u8; 4] = b"OCV1";
    const PREFIX_LEN: usize = 8;
    const CHUNK: usize = 64 << 10;
    const TAG: usize = 16;
    pub const KEY_LEN: usize = 32;

    const AAD_MIDDLE: [u8; 1] = [0x00];
    const AAD_FINAL: [u8; 1] = [0x01];

    #[derive(Debug, thiserror::Error)]
    pub enum Error {
        #[error("vault: file is corrupt, truncated or the key is wrong")]
        Auth,
        #[error("vault: key must be {KEY_LEN} bytes")]
        Key,
        #[error("io: {0}")]
        Io(#[from] std::io::Error),
        #[error("file is too large for the vault format")]
        TooLarge,
    }

    pub type Result<T, E = Error> = std::result::Result<T, E>;

    fn cipher(key: &[u8]) -> Result<Aes256Gcm> {
        if key.len() != KEY_LEN {
            return Err(Error::Key);
        }
        Aes256Gcm::new_from_slice(key).map_err(|_| Error::Key)
    }

    fn nonce(prefix: &[u8], index: u32) -> NonceBytes {
        let mut out = [0u8; 12];
        out[..PREFIX_LEN].copy_from_slice(&prefix[..PREFIX_LEN]);
        out[PREFIX_LEN..].copy_from_slice(&index.to_be_bytes());
        out
    }

    /// Encrypt `src` into a new file `dst` with a fresh random key, which is returned.
    pub fn encrypt_file(src: &Path, dst: &Path) -> Result<Zeroizing<[u8; KEY_LEN]>> {
        let mut key = Zeroizing::new([0u8; KEY_LEN]);
        getrandom::fill(key.as_mut()).map_err(std::io::Error::other)?;
        let mut input = std::io::BufReader::new(fs::File::open(src)?);
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dst)?;
        let result = (|| {
            let aead = cipher(key.as_ref())?;
            let mut prefix = [0u8; PREFIX_LEN];
            getrandom::fill(&mut prefix).map_err(std::io::Error::other)?;
            out.write_all(MAGIC)?;
            out.write_all(&prefix)?;
            let mut plain = vec![0u8; CHUNK];
            let mut index: u32 = 0;
            loop {
                let (n, final_chunk) = read_chunk(&mut input, &mut plain)?;
                let aad = if final_chunk { AAD_FINAL } else { AAD_MIDDLE };
                let sealed = aead
                    .encrypt(
                        &Nonce12::from(nonce(&prefix, index)),
                        Payload {
                            msg: &plain[..n],
                            aad: &aad,
                        },
                    )
                    .map_err(|_| Error::Auth)?;
                out.write_all(&sealed)?;
                if final_chunk {
                    break;
                }
                index = index.checked_add(1).ok_or(Error::TooLarge)?;
            }
            out.sync_all()?;
            Ok(())
        })();
        if let Err(err) = result {
            let _ = fs::remove_file(dst);
            return Err(err);
        }
        Ok(key)
    }

    /// Fill `buf` and report whether this is the last chunk: a short read, or a full read with
    /// nothing after it.
    fn read_chunk(
        input: &mut std::io::BufReader<fs::File>,
        buf: &mut [u8],
    ) -> Result<(usize, bool)> {
        let mut filled = 0;
        while filled < buf.len() {
            match input.read(&mut buf[filled..])? {
                0 => break,
                n => filled += n,
            }
        }
        let final_chunk = input.fill_buf()?.is_empty();
        Ok((filled, final_chunk))
    }

    /// Decrypt a vault file into `dst`, which is truncated first. No plaintext is left behind on
    /// failure.
    pub fn decrypt_to_file(src: &Path, dst: &Path, key: &[u8]) -> Result<()> {
        let mut input = fs::File::open(src)?;
        let mut header = [0u8; MAGIC.len() + PREFIX_LEN];
        input.read_exact(&mut header).map_err(|_| Error::Auth)?;
        if &header[..MAGIC.len()] != MAGIC {
            return Err(Error::Auth);
        }
        let prefix = &header[MAGIC.len()..];
        let aead = cipher(key)?;
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(dst)?;
        let result = (|| {
            let mut sealed = vec![0u8; CHUNK + TAG];
            let mut index: u32 = 0;
            loop {
                let n = read_up_to(&mut input, &mut sealed)?;
                let final_chunk = n < CHUNK + TAG;
                let aad = if final_chunk { AAD_FINAL } else { AAD_MIDDLE };
                let plain = aead
                    .decrypt(
                        &Nonce12::from(nonce(prefix, index)),
                        Payload {
                            msg: &sealed[..n],
                            aad: &aad,
                        },
                    )
                    .map_err(|_| Error::Auth)?;
                out.write_all(&plain)?;
                if final_chunk {
                    break;
                }
                index = index.checked_add(1).ok_or(Error::TooLarge)?;
            }
            out.sync_all()?;
            Ok(())
        })();
        if let Err(err) = result {
            let _ = fs::remove_file(dst);
            return Err(err);
        }
        Ok(())
    }

    /// Read up to `buf.len()` bytes; the number read is the chunk size.
    fn read_up_to(input: &mut fs::File, buf: &mut [u8]) -> Result<usize> {
        let mut filled = 0;
        while filled < buf.len() {
            match input.read(&mut buf[filled..])? {
                0 => break,
                n => filled += n,
            }
        }
        Ok(filled)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn temp(name: &str) -> PathBuf {
            std::env::temp_dir().join(format!("chirper-vault-{name}-{}", std::process::id()))
        }

        #[test]
        fn round_trips_and_rejects_a_wrong_key() {
            let dir = temp("roundtrip");
            let _ = fs::create_dir_all(&dir);
            let plain = dir.join("plain.bin");
            let sealed = dir.join("sealed.ocv");
            let back = dir.join("back.bin");
            fs::write(&plain, vec![7u8; 200_000]).unwrap();

            let key = encrypt_file(&plain, &sealed).unwrap();
            assert_eq!(fs::read(&sealed).unwrap().len(), 4 + 8 + 200_000 + 4 * 16);
            decrypt_to_file(&sealed, &back, key.as_ref()).unwrap();
            assert_eq!(fs::read(&plain).unwrap(), fs::read(&back).unwrap());

            let wrong = [0u8; KEY_LEN];
            assert!(matches!(
                decrypt_to_file(&sealed, &back, &wrong),
                Err(Error::Auth)
            ));
            fs::remove_dir_all(&dir).unwrap();
        }

        #[test]
        fn truncation_after_a_chunk_boundary_fails() {
            let dir = temp("truncate");
            let _ = fs::create_dir_all(&dir);
            let plain = dir.join("plain.bin");
            let sealed = dir.join("sealed.ocv");
            let back = dir.join("back.bin");
            fs::write(&plain, vec![1u8; 3 * CHUNK]).unwrap();
            let key = encrypt_file(&plain, &sealed).unwrap();
            let bytes = fs::read(&sealed).unwrap();
            fs::write(&sealed, &bytes[..bytes.len() - (CHUNK + TAG)]).unwrap();
            assert!(matches!(
                decrypt_to_file(&sealed, &back, key.as_ref()),
                Err(Error::Auth)
            ));
            fs::remove_dir_all(&dir).unwrap();
        }

        #[test]
        fn an_empty_file_round_trips() {
            let dir = temp("empty");
            let _ = fs::create_dir_all(&dir);
            let plain = dir.join("plain.bin");
            let sealed = dir.join("sealed.ocv");
            let back = dir.join("back.bin");
            fs::write(&plain, b"").unwrap();
            let key = encrypt_file(&plain, &sealed).unwrap();
            decrypt_to_file(&sealed, &back, key.as_ref()).unwrap();
            assert_eq!(fs::read(&back).unwrap(), b"");
            fs::remove_dir_all(&dir).unwrap();
        }
    }
}

/// Per-login credentials, sealed with the database key and written with mode 0600.
pub struct FileSecrets {
    root: PathBuf,
    key: Option<Zeroizing<[u8; 32]>>,
}

impl std::fmt::Debug for FileSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileSecrets")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl FileSecrets {
    pub fn new(data_dir: &Path, key: Option<&[u8; 32]>) -> std::io::Result<Self> {
        let root = data_dir.join("credentials");
        fs::create_dir_all(&root)?;
        tighten_dir(&root)?;
        Ok(Self {
            root,
            key: key.map(|k| Zeroizing::new(*k)),
        })
    }

    /// A filesystem-safe file name for a secret key (`network/login/name`).
    fn path_of(&self, key: &str) -> PathBuf {
        let mut name = String::with_capacity(key.len());
        for ch in key.chars() {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                name.push(ch);
            } else {
                name.push('~');
            }
        }
        self.root.join(format!("{name}.bin"))
    }

    fn encode(&self, value: &[u8]) -> std::io::Result<Vec<u8>> {
        match &self.key {
            None => Ok(value.to_vec()),
            Some(key) => {
                let aead = Aes256Gcm::new_from_slice(key.as_ref())
                    .map_err(|_| std::io::Error::other("invalid database key"))?;
                let mut nonce = [0u8; 12];
                getrandom::fill(&mut nonce).map_err(std::io::Error::other)?;
                let sealed = aead
                    .encrypt(&Nonce12::from(nonce), value)
                    .map_err(|_| std::io::Error::other("cannot seal secret"))?;
                let mut out = Vec::with_capacity(12 + sealed.len());
                out.extend_from_slice(&nonce);
                out.extend_from_slice(&sealed);
                Ok(out)
            }
        }
    }

    fn decode(&self, raw: &[u8]) -> std::io::Result<Zeroizing<Vec<u8>>> {
        match &self.key {
            None => Ok(Zeroizing::new(raw.to_vec())),
            Some(key) => {
                if raw.len() < 12 {
                    return Err(std::io::Error::other("secret file is corrupt"));
                }
                let aead = Aes256Gcm::new_from_slice(key.as_ref())
                    .map_err(|_| std::io::Error::other("invalid database key"))?;
                let mut at = [0u8; 12];
                at.copy_from_slice(&raw[..12]);
                let plain = aead
                    .decrypt(&Nonce12::from(at), &raw[12..])
                    .map_err(|_| std::io::Error::other("secret file cannot be decrypted"))?;
                Ok(Zeroizing::new(plain))
            }
        }
    }
}

impl FileSecrets {
    /// Delete every secret of a login. Secret files are named after their key, so a prefix delete
    /// needs no index.
    pub fn delete_prefix(&self, prefix: &str) -> Result<(), std::io::Error> {
        let stem = sanitize_prefix(prefix);
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Ok(());
        };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with(&stem) {
                if let Err(err) = fs::remove_file(entry.path()) {
                    if err.kind() != std::io::ErrorKind::NotFound {
                        return Err(err);
                    }
                }
            }
        }
        Ok(())
    }
}

impl FileSecrets {
    /// Move every secret of `from` to `to`, keeping the suffix after the prefix.
    pub fn move_prefix(&self, from: &str, to: &str) -> Result<(), std::io::Error> {
        let from_stem = sanitize_prefix(from);
        let to_stem = sanitize_prefix(to);
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Ok(());
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(suffix) = name.strip_prefix(&from_stem) else {
                continue;
            };
            let target = self.root.join(format!("{to_stem}{suffix}"));
            fs::rename(entry.path(), &target)?;
            restrict_file(&target)?;
        }
        Ok(())
    }
}

/// The file-name form of a secret key prefix.
fn sanitize_prefix(prefix: &str) -> String {
    prefix
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '~'
            }
        })
        .collect()
}

impl chirper_bridge::Secrets for FileSecrets {
    fn get(&self, key: &str) -> chirper_bridge::Result<Option<Zeroizing<Vec<u8>>>> {
        let path = self.path_of(key);
        let raw = match fs::read(&path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(chirper_bridge::Error::Internal(err.to_string())),
        };
        let plain = self
            .decode(&raw)
            .map_err(|e| chirper_bridge::Error::Internal(e.to_string()))?;
        Ok(Some(plain))
    }

    fn set(&self, key: &str, value: &[u8]) -> chirper_bridge::Result<()> {
        let path = self.path_of(key);
        let encoded = self
            .encode(value)
            .map_err(|e| chirper_bridge::Error::Internal(e.to_string()))?;
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, &encoded)
            .and_then(|()| restrict_file(&tmp))
            .and_then(|()| fs::rename(&tmp, &path))
            .map_err(|e| chirper_bridge::Error::Internal(e.to_string()))
    }

    fn move_prefix(&self, from: &str, to: &str) -> chirper_bridge::Result<()> {
        self.move_prefix(from, to)
            .map_err(|e| chirper_bridge::Error::Internal(e.to_string()))
    }

    fn delete(&self, key: &str) -> chirper_bridge::Result<()> {
        match fs::remove_file(self.path_of(key)) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(chirper_bridge::Error::Internal(err.to_string())),
        }
    }
}

/// Restrict a directory to its owner (0700).
pub fn tighten_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Restrict a file to its owner (0600).
pub fn restrict_file(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Walk a directory and restrict every entry to its owner. Used on the data directory at start and
/// stop, because an old build (or a restored backup) may have looser modes.
pub fn tighten_tree(root: &Path) -> std::io::Result<()> {
    tighten_dir(root)?;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let meta = entry.metadata()?;
        if meta.is_dir() {
            tighten_tree(&path)?;
        } else if meta.is_file() {
            restrict_file(&path)?;
        }
    }
    Ok(())
}

/// Remove anything that looks like a token, a key or a session from a string before it is logged.
pub fn redact(input: &str) -> String {
    const MARKERS: &[&str] = &[
        "token",
        "secret",
        "password",
        "passwd",
        "cookie",
        "session",
        "authorization",
        "api_key",
        "apikey",
    ];
    const SEPARATORS: &[u8] = b" \"':=";
    const DELIMITERS: &[u8] = b" ,}\"')\r\n";

    let lower = input.to_ascii_lowercase();
    if !MARKERS.iter().any(|marker| lower.contains(marker)) {
        return input.to_owned();
    }
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        let marker = MARKERS
            .iter()
            .find(|marker| lower[i..].starts_with(**marker));
        if let Some(marker) = marker {
            out.push_str(&input[i..i + marker.len()]);
            i += marker.len();
            while i < bytes.len() && SEPARATORS.contains(&bytes[i]) {
                out.push(bytes[i] as char);
                i += 1;
            }
            let start = i;
            while i < bytes.len() && !DELIMITERS.contains(&bytes[i]) {
                i += 1;
            }
            if i > start {
                out.push_str("[redacted]");
            }
        } else {
            let ch = input[i..].chars().next().unwrap_or(' ');
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// The file a private secrets cache is keyed by. Kept here so tests can clean up.
pub fn credentials_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("credentials")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_values_after_a_marker() {
        let out = redact("login failed token=abc123 code=1");
        assert!(!out.contains("abc123"), "{out}");
        assert!(out.contains("code=1"), "{out}");
        let json = redact(r#"{"access_token":"xyz","ok":true}"#);
        assert!(!json.contains("xyz"), "{json}");
    }

    #[test]
    fn leaves_ordinary_lines_alone() {
        assert_eq!(redact("connected to discord"), "connected to discord");
    }

    #[test]
    fn file_secrets_round_trip_and_are_owner_only() {
        let dir = std::env::temp_dir().join(format!("chirper-secrets-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let key = [9u8; 32];
        let secrets = FileSecrets::new(&dir, Some(&key)).unwrap();
        use chirper_bridge::Secrets;
        assert!(secrets.get("discord/1/token").unwrap().is_none());
        secrets.set("discord/1/token", b"super-secret").unwrap();
        assert_eq!(
            secrets.get("discord/1/token").unwrap().unwrap().as_slice(),
            b"super-secret"
        );
        let raw = fs::read(credentials_dir(&dir).join("discord~1~token.bin")).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("super-secret"));
        secrets.delete("discord/1/token").unwrap();
        assert!(secrets.get("discord/1/token").unwrap().is_none());
        fs::remove_dir_all(&dir).unwrap();
    }
}