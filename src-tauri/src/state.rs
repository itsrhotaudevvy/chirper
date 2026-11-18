//! Shared application state: the core handle, preferences, lock, upload allow-list and window
//! focus. It is managed by Tauri in `setup` and read by commands, events and the media protocol.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use chirper_core::Core;

use crate::error::{CmdError, CmdResult};
use crate::lock::LockMachine;
use crate::prefs::{self, Prefs};

/// How long a file returned by `pick_files` may be sent with `send_media`.
pub const UPLOAD_TTL: Duration = Duration::from_secs(10 * 60);

/// Files the user chose in a native dialog. Only these may be uploaded.
#[derive(Debug, Default)]
pub struct UploadAllowList {
    entries: HashMap<PathBuf, Instant>,
}

impl UploadAllowList {
    pub fn allow(&mut self, path: PathBuf, now: Instant) {
        self.prune(now);
        self.entries.insert(path, now);
    }

    pub fn is_allowed(&mut self, path: &Path, now: Instant) -> bool {
        self.prune(now);
        self.entries.contains_key(path)
    }

    fn prune(&mut self, now: Instant) {
        self.entries
            .retain(|_, chosen| now.duration_since(*chosen) < UPLOAD_TTL);
    }
}

pub struct AppState {
    core: OnceLock<Core>,
    core_dir: PathBuf,
    prefs_path: PathBuf,
    prefs: Mutex<Prefs>,
    lock: Mutex<LockMachine>,
    uploads: Mutex<UploadAllowList>,
    /// Deep links that arrived before the webview could listen for them.
    pending_links: Mutex<Vec<String>>,
    links_ready: AtomicBool,
    window_focused: AtomicBool,
}

fn guard<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl AppState {
    /// Creates the state for `data_dir`: loads the preferences and creates the core directory
    /// with mode 0700. The lock starts locked when the preference asks for it and the platform
    /// can authenticate.
    pub fn new(data_dir: &Path) -> std::io::Result<Self> {
        fs::create_dir_all(data_dir)?;
        let core_dir = data_dir.join("core");
        fs::create_dir_all(&core_dir)?;
        restrict_dir(&core_dir)?;
        let core_dir = fs::canonicalize(&core_dir)?;

        let prefs_path = data_dir.join(prefs::PREFS_FILE);
        let prefs = prefs::load(&prefs_path);
        let wants_lock = prefs.app_lock;
        let can_lock = crate::platform::app_lock_supported();
        if wants_lock && !can_lock {
            log::warn!("app lock is set but not supported on this platform; not locking");
        }
        let lock = LockMachine::new(wants_lock && can_lock, prefs.lock_after_secs);
        Ok(Self {
            core: OnceLock::new(),
            core_dir,
            prefs_path,
            prefs: Mutex::new(prefs),
            lock: Mutex::new(lock),
            uploads: Mutex::new(UploadAllowList::default()),
            pending_links: Mutex::new(Vec::new()),
            links_ready: AtomicBool::new(false),
            window_focused: AtomicBool::new(false),
        })
    }

    pub fn core_dir(&self) -> &Path {
        &self.core_dir
    }

    pub fn set_core(&self, core: Core) -> Result<(), Core> {
        self.core.set(core)
    }

    /// The core, when the app is unlocked and the core has opened.
    pub fn ready_core(&self) -> CmdResult<Core> {
        self.ensure_unlocked()?;
        self.core.get().cloned().ok_or_else(CmdError::not_ready)
    }

    pub fn close_core(&self) {
        if let Some(core) = self.core.get() {
            core.shutdown();
        }
    }

    pub fn ensure_unlocked(&self) -> CmdResult<()> {
        if guard(&self.lock).is_locked() {
            Err(CmdError::locked())
        } else {
            Ok(())
        }
    }

    pub fn lock_machine(&self) -> MutexGuard<'_, LockMachine> {
        guard(&self.lock)
    }

    pub fn prefs(&self) -> Prefs {
        guard(&self.prefs).clone()
    }

    /// Stores new preferences and writes them to disk.
    pub fn store_prefs(&self, next: Prefs) -> std::io::Result<()> {
        prefs::save(&self.prefs_path, &next)?;
        *guard(&self.prefs) = next;
        Ok(())
    }

    pub fn allow_upload(&self, path: PathBuf) {
        guard(&self.uploads).allow(path, Instant::now());
    }

    pub fn is_upload_allowed(&self, path: &Path) -> bool {
        guard(&self.uploads).is_allowed(path, Instant::now())
    }

    pub fn set_window_focused(&self, focused: bool) {
        self.window_focused.store(focused, Ordering::SeqCst);
    }

    pub fn window_focused(&self) -> bool {
        self.window_focused.load(Ordering::SeqCst)
    }

    /// Queues a deep link until the webview can receive it, or returns it for immediate delivery.
    pub fn queue_or_take_link(&self, url: String) -> Option<String> {
        if self.links_ready.load(Ordering::SeqCst) {
            return Some(url);
        }
        guard(&self.pending_links).push(url);
        None
    }

    /// Marks the webview as listening and returns the links queued so far.
    pub fn take_pending_links(&self) -> Vec<String> {
        self.links_ready.store(true, Ordering::SeqCst);
        std::mem::take(&mut *guard(&self.pending_links))
    }
}

#[cfg(unix)]
fn restrict_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn restrict_dir(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_allow_list_expires_after_ttl() {
        let t0 = Instant::now();
        let mut list = UploadAllowList::default();
        let path = PathBuf::from("/tmp/a.jpg");
        list.allow(path.clone(), t0);
        assert!(list.is_allowed(&path, t0 + Duration::from_secs(599)));
        assert!(!list.is_allowed(&path, t0 + UPLOAD_TTL));
    }

    #[test]
    fn upload_allow_list_only_contains_chosen_paths() {
        let t0 = Instant::now();
        let mut list = UploadAllowList::default();
        list.allow(PathBuf::from("/tmp/a.jpg"), t0);
        assert!(!list.is_allowed(Path::new("/tmp/b.jpg"), t0));
    }
}
