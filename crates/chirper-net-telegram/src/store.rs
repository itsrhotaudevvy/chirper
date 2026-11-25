//! Persistence of one login.
//!
//! Secrets go through the core's secret store (`LoginCtx::{get,set}_secret`): the auth key of each
//! data centre (`auth_key/<dc>`, 256 bytes) and the home data centre (`home_dc`). Everything else
//! lives in `state.json` in the login's data directory: per-DC salts and time offsets, the update
//! position, channel `pts` and a bounded peer cache (access hashes and minimal names). The file is
//! written atomically (temporary file, then rename) with mode 0600, and never holds a secret.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::Mutex;

use chirper_bridge::{Error, LoginCtx, Result};
use serde::{Deserialize, Serialize};

use crate::mtproto::AuthKey;
use crate::state::Position;

/// Most peers kept in the cache; the least recently seen are dropped first.
pub const PEER_CAP: usize = 50_000;

/// The file name of the non-secret state.
const STATE_FILE: &str = "state.json";

/// Salt and time offset of one data centre connection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DcState {
    pub server_salt: i64,
    pub time_offset: i64,
}

/// A user or channel the account has seen, with what the API needs to address it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerEntry {
    pub access_hash: i64,
    /// Display name, minimal: first and last name, or the title.
    pub name: String,
    pub username: String,
}

/// The on-disk form of `state.json`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Disk {
    #[serde(default)]
    pub dcs: BTreeMap<i32, DcState>,
    #[serde(default)]
    pub position: Option<Position>,
    #[serde(default)]
    pub channels: BTreeMap<i64, i32>,
    /// Keys `u:<id>` for users and `c:<id>` for channels.
    #[serde(default)]
    pub peers: BTreeMap<String, PeerEntry>,
}

/// The peer cache's recency index, rebuilt from the file on load.
#[derive(Debug, Default)]
struct Recency {
    next: u64,
    /// Recency counter to key, oldest first.
    order: BTreeMap<u64, String>,
    /// Key to its counter.
    of: BTreeMap<String, u64>,
}

impl Recency {
    fn touch(&mut self, key: &str) {
        if let Some(old) = self.of.remove(key) {
            self.order.remove(&old);
        }
        self.next += 1;
        self.order.insert(self.next, key.to_owned());
        self.of.insert(key.to_owned(), self.next);
    }

    /// Forget the oldest keys until at most `cap` remain; returns the forgotten keys.
    fn evict(&mut self, cap: usize) -> Vec<String> {
        let mut dropped = Vec::new();
        while self.of.len() > cap {
            let Some((_, key)) = self.order.pop_first() else {
                break;
            };
            self.of.remove(&key);
            dropped.push(key);
        }
        dropped
    }
}

struct Inner {
    disk: Disk,
    recency: Recency,
    dirty: bool,
}

/// The state of one login, shared by its client and session.
pub struct Store {
    ctx: LoginCtx,
    path: PathBuf,
    inner: Mutex<Inner>,
    cap: usize,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

fn storage(message: &str) -> Error {
    Error::Internal(format!("telegram state: {message}"))
}

impl Store {
    /// Open the state of a login. A missing file is an empty state; an unreadable one is an empty
    /// state too, and the next flush replaces it.
    pub fn open(ctx: LoginCtx) -> Store {
        Self::open_with_cap(ctx, PEER_CAP)
    }

    fn open_with_cap(ctx: LoginCtx, cap: usize) -> Store {
        let path = ctx.data_dir.join(STATE_FILE);
        let disk: Disk = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                log::warn!("telegram state file is unreadable; starting from an empty state");
                Disk::default()
            }),
            Err(_) => Disk::default(),
        };
        let mut recency = Recency::default();
        for key in disk.peers.keys() {
            recency.touch(key);
        }
        let mut store = Store {
            ctx,
            path,
            inner: Mutex::new(Inner {
                disk,
                recency,
                dirty: false,
            }),
            cap,
        };
        store.enforce_cap();
        store
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn enforce_cap(&mut self) {
        let cap = self.cap;
        let inner = self
            .inner
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for key in inner.recency.evict(cap) {
            inner.disk.peers.remove(&key);
            inner.dirty = true;
        }
    }

    /// The auth key of a data centre, if one is stored.
    pub fn auth_key(&self, dc: i32) -> Result<Option<AuthKey>> {
        let Some(bytes) = self.ctx.get_secret(&format!("auth_key/{dc}"))? else {
            return Ok(None);
        };
        let array: [u8; 256] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| storage("stored auth key has the wrong length"))?;
        Ok(Some(AuthKey::from_bytes(array)))
    }

    /// Store the auth key of a data centre.
    pub fn save_auth_key(&self, dc: i32, key: &AuthKey) -> Result<()> {
        self.ctx
            .set_secret(&format!("auth_key/{dc}"), key.as_bytes())
    }

    /// The data centre the account lives in, if it is known.
    pub fn home_dc(&self) -> Result<Option<i32>> {
        let Some(bytes) = self.ctx.get_secret("home_dc")? else {
            return Ok(None);
        };
        let text = std::str::from_utf8(&bytes).map_err(|_| storage("home dc is not text"))?;
        Ok(text.trim().parse().ok())
    }

    /// Remember the home data centre.
    pub fn save_home_dc(&self, dc: i32) -> Result<()> {
        self.ctx.set_secret("home_dc", dc.to_string().as_bytes())
    }

    /// Salt and time offset last used with a data centre.
    pub fn dc_state(&self, dc: i32) -> DcState {
        self.lock().disk.dcs.get(&dc).copied().unwrap_or_default()
    }

    /// Record the salt and time offset of a data centre connection.
    pub fn set_dc_state(&self, dc: i32, state: DcState) {
        let mut inner = self.lock();
        inner.disk.dcs.insert(dc, state);
        inner.dirty = true;
    }

    /// The common update position, or `None` before the first `updates.getState`.
    pub fn position(&self) -> Option<Position> {
        self.lock().disk.position
    }

    /// Replace the common update position.
    pub fn set_position(&self, position: Position) {
        let mut inner = self.lock();
        inner.disk.position = Some(position);
        inner.dirty = true;
    }

    /// The stored `pts` of a channel.
    pub fn channel_pts(&self, channel: i64) -> Option<i32> {
        self.lock().disk.channels.get(&channel).copied()
    }

    /// Replace the stored `pts` of a channel.
    pub fn set_channel_pts(&self, channel: i64, pts: i32) {
        let mut inner = self.lock();
        inner.disk.channels.insert(channel, pts);
        inner.dirty = true;
    }

    /// Forget a channel's `pts` (after it was left or its state was reset).
    pub fn forget_channel(&self, channel: i64) {
        let mut inner = self.lock();
        if inner.disk.channels.remove(&channel).is_some() {
            inner.dirty = true;
        }
    }

    /// Forget every channel `pts` and the update position, for a full resync.
    pub fn reset_updates(&self) {
        let mut inner = self.lock();
        inner.disk.channels.clear();
        inner.disk.position = None;
        inner.dirty = true;
    }

    /// The peer cache entry for a key (`u:<id>` or `c:<id>`).
    pub fn peer(&self, key: &str) -> Option<PeerEntry> {
        self.lock().disk.peers.get(key).cloned()
    }

    /// Insert or refresh a peer, evicting the least recently seen when the cache is full.
    pub fn remember_peer(&self, key: String, entry: PeerEntry) {
        let mut inner = self.lock();
        let inner = &mut *inner;
        inner.disk.peers.insert(key.clone(), entry);
        inner.recency.touch(&key);
        for dropped in inner.recency.evict(self.cap) {
            inner.disk.peers.remove(&dropped);
        }
        inner.dirty = true;
    }

    /// Number of cached peers.
    #[cfg(test)]
    pub fn peer_count(&self) -> usize {
        self.lock().disk.peers.len()
    }

    /// Write `state.json` if anything changed since the last write.
    pub fn flush(&self) -> Result<()> {
        let bytes = {
            let mut inner = self.lock();
            if !inner.dirty {
                return Ok(());
            }
            inner.dirty = false;
            serde_json::to_vec(&inner.disk).map_err(|_| storage("could not encode the state"))?
        };
        write_atomic(&self.path, &bytes).map_err(|_| storage("could not write the state file"))
    }
}

/// Write `bytes` to `path` through a temporary file with mode 0600, then rename it into place.
pub fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex as StdMutex};

    use chirper_bridge::{Result as BridgeResult, Secrets};
    use zeroize::Zeroizing;

    use super::*;

    /// An in-memory secret store.
    #[derive(Default)]
    struct MemorySecrets(StdMutex<BTreeMap<String, Vec<u8>>>);

    impl Secrets for MemorySecrets {
        fn get(&self, key: &str) -> BridgeResult<Option<Zeroizing<Vec<u8>>>> {
            let map = self
                .0
                .lock()
                .map_err(|_| Error::Internal("poisoned".into()))?;
            Ok(map.get(key).map(|v| Zeroizing::new(v.clone())))
        }
        fn set(&self, key: &str, value: &[u8]) -> BridgeResult<()> {
            let mut map = self
                .0
                .lock()
                .map_err(|_| Error::Internal("poisoned".into()))?;
            map.insert(key.to_owned(), value.to_vec());
            Ok(())
        }
        fn delete(&self, key: &str) -> BridgeResult<()> {
            let mut map = self
                .0
                .lock()
                .map_err(|_| Error::Internal("poisoned".into()))?;
            map.remove(key);
            Ok(())
        }
    }

    /// A fresh, empty data directory under the system temp directory.
    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "chirper-tg-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn ctx(dir: PathBuf, secrets: Arc<MemorySecrets>) -> LoginCtx {
        LoginCtx {
            network: "telegram".into(),
            login_id: "1".into(),
            data_dir: dir,
            secrets,
            http: chirper_bridge::http_client(),
        }
    }

    #[test]
    fn peer_cache_is_bounded_and_evicts_the_oldest() {
        let dir = scratch_dir("cap");
        let mut store = Store::open_with_cap(ctx(dir.clone(), Arc::default()), 3);
        for id in 1..=5 {
            store.remember_peer(format!("u:{id}"), PeerEntry::default());
        }
        store.enforce_cap();
        assert_eq!(store.peer_count(), 3);
        assert!(store.peer("u:1").is_none());
        assert!(store.peer("u:5").is_some());
        // Touching an old entry keeps it.
        store.remember_peer(
            "u:3".into(),
            PeerEntry {
                access_hash: 9,
                ..PeerEntry::default()
            },
        );
        store.remember_peer("u:6".into(), PeerEntry::default());
        assert!(store.peer("u:3").is_some());
        assert!(store.peer("u:4").is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn state_file_round_trips_without_secrets() {
        let dir = scratch_dir("state");
        let store = Store::open(ctx(dir.clone(), Arc::default()));
        store.set_position(Position {
            pts: 5,
            qts: 1,
            date: 2,
            seq: 3,
        });
        store.set_channel_pts(77, 12);
        store.set_dc_state(
            2,
            DcState {
                server_salt: -4,
                time_offset: 9,
            },
        );
        store.remember_peer(
            "c:77".into(),
            PeerEntry {
                access_hash: 3,
                name: "News".into(),
                username: String::new(),
            },
        );
        store.flush().unwrap();
        let reopened = Store::open(ctx(dir.clone(), Arc::default()));
        assert_eq!(reopened.position().map(|p| p.pts), Some(5));
        assert_eq!(reopened.channel_pts(77), Some(12));
        assert_eq!(reopened.dc_state(2).time_offset, 9);
        assert_eq!(reopened.peer("c:77").map(|p| p.access_hash), Some(3));
        let text = fs::read_to_string(dir.join(STATE_FILE)).unwrap();
        assert!(!text.contains("auth"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn state_file_is_written_with_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch_dir("mode");
        let store = Store::open(ctx(dir.clone(), Arc::default()));
        store.set_channel_pts(1, 1);
        store.flush().unwrap();
        let mode = fs::metadata(dir.join(STATE_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert!(!dir.join("state.json.tmp").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn auth_keys_and_home_dc_live_in_secrets() {
        let dir = scratch_dir("secrets");
        let secrets: Arc<MemorySecrets> = Arc::default();
        let store = Store::open(ctx(dir.clone(), secrets.clone()));
        assert!(store.auth_key(2).unwrap().is_none());
        store
            .save_auth_key(2, &AuthKey::from_bytes([7u8; 256]))
            .unwrap();
        assert_eq!(
            store.auth_key(2).unwrap().map(|k| *k.as_bytes()),
            Some([7u8; 256])
        );
        store.save_home_dc(4).unwrap();
        assert_eq!(store.home_dc().unwrap(), Some(4));
        assert!(
            secrets
                .0
                .lock()
                .unwrap()
                .contains_key("telegram/1/auth_key/2")
        );
        let _ = fs::remove_dir_all(dir);
    }
}
