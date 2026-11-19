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

use aes_gcm::Aes256Gcm;
use aes_gcm::aead::{Aead, KeyInit, Payload};

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
        let mut out = create_private_file(dst, true)?;
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
        let mut out = create_private_file(dst, false)?;
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
        create_private_dir(&root)?;
        Ok(Self {
            root,
            key: key.map(|k| Zeroizing::new(*k)),
        })
    }

    /// The file a secret key is stored in.
    fn path_of(&self, key: &str) -> PathBuf {
        self.root.join(format!("{}.bin", file_stem(key)))
    }

    /// The file a key had under the scheme that mapped every other character to `~`. Only differs
    /// from [`path_of`](Self::path_of) for keys with such characters, and is read as a fallback.
    fn legacy_path_of(&self, key: &str) -> PathBuf {
        self.root.join(format!("{}.bin", legacy_stem(key)))
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
    /// Delete every secret whose key starts with `prefix`. The prefix is compared on whole keys, so
    /// `discord/1/` never reaches `discord/12/`.
    pub fn delete_prefix(&self, prefix: &str) -> Result<(), std::io::Error> {
        for (name, file) in self.entries() {
            if matches_prefix(&name, prefix)
                && let Err(err) = fs::remove_file(file)
                && err.kind() != std::io::ErrorKind::NotFound
            {
                return Err(err);
            }
        }
        Ok(())
    }

    /// Move every secret of `from` to `to`, keeping the suffix after the prefix.
    pub fn move_prefix(&self, from: &str, to: &str) -> Result<(), std::io::Error> {
        for (name, file) in self.entries() {
            if !matches_prefix(&name, from) {
                continue;
            }
            let suffix = suffix_after(&name, from);
            let target = self.path_of(&format!("{to}{suffix}"));
            fs::rename(&file, &target)?;
            restrict_file(&target)?;
        }
        Ok(())
    }

    /// Every secret file, as `(key, path)`. The key is the decoded name, or the raw legacy name when
    /// the file predates the current scheme; [`matches_prefix`] knows how to compare the two.
    fn entries(&self) -> Vec<(String, PathBuf)> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|entry| {
                let file = entry.file_name().to_string_lossy().into_owned();
                let stem = file.strip_suffix(".bin")?.to_owned();
                Some((stem, entry.path()))
            })
            .collect()
    }
}

/// Whether the secret stored under file stem `stem` belongs to a key that starts with `prefix`.
fn matches_prefix(stem: &str, prefix: &str) -> bool {
    if stem.contains('%') {
        return decode_stem(stem).starts_with(prefix);
    }
    // Clean and legacy names carry `~` where a `/` was; compare on that form.
    stem.starts_with(&legacy_stem(prefix))
}

/// The part of a stem's key after `prefix`, in the form [`file_stem`] expects.
fn suffix_after(stem: &str, prefix: &str) -> String {
    if stem.contains('%') {
        return decode_stem(stem)
            .strip_prefix(prefix)
            .unwrap_or_default()
            .to_owned();
    }
    stem.get(legacy_stem(prefix).len()..)
        .unwrap_or_default()
        .replace('~', "/")
}

fn read_optional(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(raw) => Ok(Some(raw)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// Write `bytes` to a new owner-only file. The mode is set at creation, so there is no moment
/// when the file is readable by others.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = create_private_file(path, false)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// A key is "clean" when every character is a letter, digit, `-`, `_`, `.` or `/`. Clean keys keep
/// the file name they always had (`/` becomes `~`), which is injective for them: `~` cannot occur
/// in a clean key.
fn is_clean(key: &str) -> bool {
    key.chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/'))
}

/// The file stem of a secret key. Keys that are not clean are percent-encoded byte by byte, so
/// distinct keys never share a file and no key can name a path. Percent-encoded stems contain `%`,
/// clean ones never do, so the two forms cannot collide.
fn file_stem(key: &str) -> String {
    if is_clean(key) {
        return key.replace('/', "~");
    }
    let mut out = String::with_capacity(key.len() * 3);
    for byte in key.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The stem the scheme before percent-encoding used: every character outside the safe set became
/// `~`. Equal to [`file_stem`] for clean keys.
fn legacy_stem(key: &str) -> String {
    key.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '~'
            }
        })
        .collect()
}

/// The key a percent-encoded stem stands for.
fn decode_stem(stem: &str) -> String {
    let bytes = stem.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let decoded = stem
                .get(i + 1..i + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok());
            match decoded {
                Some(byte) => {
                    out.push(byte);
                    i += 3;
                }
                None => {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl chirper_bridge::Secrets for FileSecrets {
    fn get(&self, key: &str) -> chirper_bridge::Result<Option<Zeroizing<Vec<u8>>>> {
        let path = self.path_of(key);
        let legacy = self.legacy_path_of(key);
        let internal = |err: std::io::Error| chirper_bridge::Error::Internal(err.to_string());
        let (raw, from) = match read_optional(&path).map_err(internal)? {
            Some(raw) => (raw, path.clone()),
            None => match read_optional(&legacy).map_err(internal)? {
                Some(raw) => (raw, legacy.clone()),
                None => return Ok(None),
            },
        };
        let plain = self
            .decode(&raw)
            .map_err(|e| chirper_bridge::Error::Internal(e.to_string()))?;
        if from != path {
            // Migrate on read: the secret moves to the name the current scheme uses.
            self.set(key, &plain)?;
            let _ = fs::remove_file(&from);
        }
        Ok(Some(plain))
    }

    fn set(&self, key: &str, value: &[u8]) -> chirper_bridge::Result<()> {
        let path = self.path_of(key);
        let encoded = self
            .encode(value)
            .map_err(|e| chirper_bridge::Error::Internal(e.to_string()))?;
        let tmp = path.with_extension("tmp");
        write_private(&tmp, &encoded)
            .and_then(|()| fs::rename(&tmp, &path))
            .map_err(|e| chirper_bridge::Error::Internal(e.to_string()))
    }

    fn move_prefix(&self, from: &str, to: &str) -> chirper_bridge::Result<()> {
        self.move_prefix(from, to)
            .map_err(|e| chirper_bridge::Error::Internal(e.to_string()))
    }

    fn delete(&self, key: &str) -> chirper_bridge::Result<()> {
        for path in [self.path_of(key), self.legacy_path_of(key)] {
            if let Err(err) = fs::remove_file(path)
                && err.kind() != std::io::ErrorKind::NotFound
            {
                return Err(chirper_bridge::Error::Internal(err.to_string()));
            }
        }
        Ok(())
    }
}

/// Create a directory, and any missing parent, owner-only from the start (0700 on unix).
pub fn create_private_dir(path: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    tighten_dir(path)
}

/// Open a file for writing, owner-only from the start (0600 on unix). With `create_new` the file
/// must not exist; otherwise an existing file is truncated.
pub fn create_private_file(path: &Path, create_new: bool) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true).truncate(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Make sure a file exists and is owner-only, without touching its contents.
pub fn ensure_private_file(path: &Path) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?;
    restrict_file(path)
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
pub use chirper_bridge::redact;

/// The file a private secrets cache is keyed by. Kept here so tests can clean up.
pub fn credentials_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("credentials")
}

#[cfg(test)]
mod tests {
    use super::*;

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

#[cfg(test)]
mod hardening_tests {
    use super::*;
    use chirper_bridge::Secrets;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("chirper-harden-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn files_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn distinct_keys_never_share_a_secret_file() {
        let dir = scratch("collide");
        let secrets = FileSecrets::new(&dir, None).unwrap();
        // Under the old scheme all three names became `net~a~b~token.bin`.
        for (key, value) in [
            ("net/a/b/token", "slash"),
            ("net/a~b/token", "tilde"),
            ("net/a:b/token", "colon"),
        ] {
            secrets.set(key, value.as_bytes()).unwrap();
        }
        for (key, value) in [
            ("net/a/b/token", "slash"),
            ("net/a~b/token", "tilde"),
            ("net/a:b/token", "colon"),
        ] {
            assert_eq!(
                secrets.get(key).unwrap().unwrap().as_slice(),
                value.as_bytes(),
                "{key}"
            );
        }
        assert_eq!(files_in(&credentials_dir(&dir)).len(), 3);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_hostile_key_stays_inside_the_credentials_directory() {
        let dir = scratch("traversal");
        let secrets = FileSecrets::new(&dir, None).unwrap();
        let hostile = "net/../../../escape\\..\\..\0/token";
        secrets.set(hostile, b"contained").unwrap();
        assert_eq!(
            secrets.get(hostile).unwrap().unwrap().as_slice(),
            b"contained"
        );
        assert_eq!(files_in(&dir), vec!["credentials".to_owned()]);
        let entries = files_in(&credentials_dir(&dir));
        assert_eq!(entries.len(), 1, "{entries:?}");
        assert!(!entries[0].contains('/') && !entries[0].contains('\\'));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_secret_written_under_the_legacy_name_is_read_and_migrated() {
        let dir = scratch("legacy");
        let secrets = FileSecrets::new(&dir, None).unwrap();
        // The file the old scheme wrote for `net/a:b/token`.
        let legacy = credentials_dir(&dir).join("net~a~b~token.bin");
        fs::write(&legacy, b"old-secret").unwrap();

        let got = secrets.get("net/a:b/token").unwrap().unwrap();
        assert_eq!(got.as_slice(), b"old-secret");
        assert!(!legacy.exists(), "the legacy file should have moved");
        assert_eq!(
            secrets.get("net/a:b/token").unwrap().unwrap().as_slice(),
            b"old-secret"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clean_keys_keep_the_file_names_they_always_had() {
        let dir = scratch("clean");
        let secrets = FileSecrets::new(&dir, None).unwrap();
        secrets.set("discord/123/token", b"x").unwrap();
        assert_eq!(
            files_in(&credentials_dir(&dir)),
            vec!["discord~123~token.bin".to_owned()]
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prefix_operations_compare_whole_keys() {
        let dir = scratch("prefix");
        let secrets = FileSecrets::new(&dir, None).unwrap();
        secrets.set("net/1/token", b"one").unwrap();
        secrets.set("net/12/token", b"twelve").unwrap();
        secrets.set("net/1:x/token", b"odd").unwrap();

        secrets.delete_prefix("net/1/").unwrap();
        assert!(secrets.get("net/1/token").unwrap().is_none());
        assert_eq!(
            secrets.get("net/12/token").unwrap().unwrap().as_slice(),
            b"twelve",
            "deleting login 1 must not touch login 12"
        );
        assert_eq!(
            secrets.get("net/1:x/token").unwrap().unwrap().as_slice(),
            b"odd"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn move_prefix_moves_one_login_and_keeps_the_suffix() {
        let dir = scratch("move");
        let secrets = FileSecrets::new(&dir, None).unwrap();
        secrets.set("net/provisional/token", b"t").unwrap();
        secrets.set("net/provisional/cookie", b"c").unwrap();
        secrets.set("net/provisional2/token", b"other").unwrap();

        secrets
            .move_prefix("net/provisional/", "net/final/")
            .unwrap();
        assert_eq!(
            secrets.get("net/final/token").unwrap().unwrap().as_slice(),
            b"t"
        );
        assert_eq!(
            secrets.get("net/final/cookie").unwrap().unwrap().as_slice(),
            b"c"
        );
        assert!(secrets.get("net/provisional/token").unwrap().is_none());
        assert_eq!(
            secrets
                .get("net/provisional2/token")
                .unwrap()
                .unwrap()
                .as_slice(),
            b"other"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn secret_files_and_directories_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("modes");
        let secrets = FileSecrets::new(&dir, None).unwrap();
        secrets.set("net/1/token", b"x").unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&credentials_dir(&dir)), 0o700);
        assert_eq!(mode(&credentials_dir(&dir).join("net~1~token.bin")), 0o600);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn private_files_are_owner_only_from_creation() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("private");
        let path = dir.join("made.bin");
        let file = create_private_file(&path, true).unwrap();
        drop(file);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(
            create_private_file(&path, true).is_err(),
            "create_new must refuse"
        );

        let sub = dir.join("a").join("b");
        create_private_dir(&sub).unwrap();
        assert_eq!(
            fs::metadata(&sub).unwrap().permissions().mode() & 0o777,
            0o700
        );
        ensure_private_file(&dir.join("db")).unwrap();
        fs::write(dir.join("db"), b"keep me").unwrap();
        ensure_private_file(&dir.join("db")).unwrap();
        assert_eq!(fs::read(dir.join("db")).unwrap(), b"keep me");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn vault_output_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("vault-mode");
        let plain = dir.join("plain.bin");
        let sealed = dir.join("sealed.ocv");
        fs::write(&plain, b"hello").unwrap();
        let key = vault::encrypt_file(&plain, &sealed).unwrap();
        assert_eq!(
            fs::metadata(&sealed).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let back = dir.join("back.bin");
        vault::decrypt_to_file(&sealed, &back, key.as_ref()).unwrap();
        assert_eq!(
            fs::metadata(&back).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
