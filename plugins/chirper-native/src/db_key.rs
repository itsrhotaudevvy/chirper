//! The 32-byte database key.
//!
//! Android: the key is random, generated here, and stored wrapped by a non-exportable AES-256-GCM key
//! in the Android Keystore. The wrapped form is a file in the no-backup directory (see `keyfile`).
//! iOS: the key is stored as a Keychain item that is `ThisDeviceOnly` and readable after the first
//! unlock, so the background refresh can open the database before the next unlock.
//!
//! A missing key is created. An unreadable key is an error: replacing it would make the existing
//! database unrecoverable.

use tauri::{Manager, Runtime};
use zeroize::Zeroizing;

use crate::mobile::{ChirperNative, ChirperNativeExt};
use crate::{Error, Result};

pub const DB_KEY_LEN: usize = 32;

#[cfg(target_os = "android")]
const KEY_FILE: &str = "db-key.v1";
#[cfg(target_os = "ios")]
const KEYCHAIN_SERVICE: &str = "app.chirper";
#[cfg(target_os = "ios")]
const KEYCHAIN_ACCOUNT: &str = "db-key";

type Key = Zeroizing<[u8; DB_KEY_LEN]>;

/// Returns the database key of the app, creating it on first use.
pub fn load_or_create_db_key<R: Runtime>(app: &impl Manager<R>) -> Result<Key> {
    let native = app.chirper_native();
    #[cfg(target_os = "android")]
    {
        android::load_or_create(native)
    }
    #[cfg(target_os = "ios")]
    {
        ios::load_or_create(native)
    }
}

fn random_key() -> Result<Key> {
    let mut key = Zeroizing::new([0u8; DB_KEY_LEN]);
    getrandom::fill(&mut key[..]).map_err(|_| Error::Random)?;
    Ok(key)
}

fn key_from_bytes(bytes: &[u8]) -> Result<Key> {
    let array: [u8; DB_KEY_LEN] = bytes
        .try_into()
        .map_err(|_| Error::Invalid("the stored database key has the wrong length"))?;
    Ok(Zeroizing::new(array))
}

#[cfg(target_os = "android")]
mod android {
    use std::fs::{self, OpenOptions};
    use std::io::{ErrorKind, Write};
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::Path;

    use super::*;
    use crate::keyfile;
    use crate::mobile::Wrapped;

    pub(super) fn load_or_create<R: Runtime>(native: &ChirperNative<R>) -> Result<Key> {
        let path = native.no_backup_dir()?.join(KEY_FILE);
        match fs::read(&path) {
            Ok(bytes) => {
                let (iv, ciphertext) = keyfile::decode(&bytes)?;
                let plaintext = match native.unwrap_key(&Wrapped {
                    iv: iv.to_vec(),
                    ciphertext: ciphertext.to_vec(),
                }) {
                    Ok(p) => p,
                    Err(e) => {
                        log::warn!("failed to unwrap database key ({e}), creating a fresh key");
                        let _ = fs::remove_file(&path);
                        return create_and_store(native, &path);
                    }
                };
                key_from_bytes(&plaintext)
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {
                create_and_store(native, &path)
            }
            Err(e) => Err(e.into()),
        }
    }

    fn create_and_store<R: Runtime>(native: &ChirperNative<R>, path: &Path) -> Result<Key> {
        let key = random_key()?;
        let wrapped = native.wrap_key(&key[..])?;
        let file = keyfile::encode(&wrapped.iv, &wrapped.ciphertext)?;
        write_private(path, &file)?;
        Ok(key)
    }

    /// Writes through a temporary file with mode 0600, then renames it into place.
    fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
        let tmp = path.with_extension("tmp");
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(target_os = "ios")]
mod ios {
    use super::*;

    pub(super) fn load_or_create<R: Runtime>(native: &ChirperNative<R>) -> Result<Key> {
        let candidate = random_key()?;
        let stored =
            native.keychain_store_if_absent(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT, &candidate[..])?;
        key_from_bytes(&stored)
    }
}
