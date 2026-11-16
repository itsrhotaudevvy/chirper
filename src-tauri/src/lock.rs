//! App lock: a pure state machine plus the app-level hooks that drive it.
//!
//! The app starts locked when `appLock` is on. It locks again once the window has been in the
//! background for longer than `lockAfterSecs`. Unlocking needs the platform authentication.

use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};

use crate::error::{CmdError, CmdResult};
use crate::state::AppState;

pub const EVENT_APP_LOCK: &str = "app-lock";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockMachine {
    enabled: bool,
    lock_after: Duration,
    locked: bool,
    backgrounded_at: Option<Instant>,
    external_ui: bool,
}

impl LockMachine {
    /// Starts locked when `enabled`.
    pub fn new(enabled: bool, lock_after_secs: u64) -> Self {
        Self {
            enabled,
            lock_after: Duration::from_secs(lock_after_secs),
            locked: enabled,
            backgrounded_at: None,
            external_ui: false,
        }
    }

    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// Applies new settings. Turning the lock off unlocks; turning it on does not lock at once.
    pub fn configure(&mut self, enabled: bool, lock_after_secs: u64) {
        self.enabled = enabled;
        self.lock_after = Duration::from_secs(lock_after_secs);
        if !enabled {
            self.locked = false;
            self.backgrounded_at = None;
        }
    }

    /// The window lost focus or was suspended. Returns nothing: the lock applies on return.
    pub fn background(&mut self, now: Instant) {
        if self.enabled && !self.external_ui && self.backgrounded_at.is_none() {
            self.backgrounded_at = Some(now);
        }
    }

    /// The window is back. Returns `Some(true)` when this call locked the app.
    pub fn foreground(&mut self, now: Instant) -> Option<bool> {
        let since = self.backgrounded_at.take()?;
        if self.enabled && !self.locked && now.duration_since(since) >= self.lock_after {
            self.locked = true;
            return Some(true);
        }
        None
    }

    /// Marks the app unlocked after a successful authentication. Returns whether it changed.
    pub fn unlock(&mut self) -> bool {
        let changed = self.locked;
        self.locked = false;
        self.backgrounded_at = None;
        changed
    }

    /// A native picker is open: its focus loss is not a background period.
    pub fn set_external_ui(&mut self, open: bool) {
        self.external_ui = open;
        if open {
            self.backgrounded_at = None;
        }
    }
}

fn emit_state(app: &AppHandle, locked: bool) {
    if let Err(e) = app.emit(EVENT_APP_LOCK, serde_json::json!({ "locked": locked })) {
        log::warn!("cannot emit app-lock: {e}");
    }
}

/// The window lost focus or was suspended.
pub fn background(app: &AppHandle) {
    app.state::<AppState>()
        .lock_machine()
        .background(Instant::now());
}

/// The window is in front again.
pub fn foreground(app: &AppHandle) {
    let state = app.state::<AppState>();
    let locked = state.lock_machine().foreground(Instant::now());
    if let Some(locked) = locked {
        emit_state(app, locked);
    }
}

pub fn is_locked(app: &AppHandle) -> bool {
    app.state::<AppState>().lock_machine().is_locked()
}

/// Asks the platform to authenticate and unlocks on success. Returns the new lock state.
pub async fn unlock(app: &AppHandle) -> CmdResult<bool> {
    if !is_locked(app) {
        return Ok(true);
    }
    let handle = app.clone();
    let authenticated = tauri::async_runtime::spawn_blocking(move || {
        crate::platform::authenticate(&handle, "Unlock Chirper")
    })
    .await
    .map_err(|e| CmdError::new(crate::error::code::CORE, e.to_string()))??;
    if authenticated {
        let changed = app.state::<AppState>().lock_machine().unlock();
        if changed {
            emit_state(app, false);
        }
    }
    Ok(!is_locked(app))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_locked_only_when_enabled() {
        assert!(LockMachine::new(true, 60).is_locked());
        assert!(!LockMachine::new(false, 60).is_locked());
    }

    #[test]
    fn locks_after_the_background_period() {
        let t0 = Instant::now();
        let mut m = LockMachine::new(true, 60);
        assert!(m.unlock());
        m.background(t0);
        assert_eq!(m.foreground(t0 + Duration::from_secs(59)), None);
        assert!(!m.is_locked());

        m.background(t0);
        assert_eq!(m.foreground(t0 + Duration::from_secs(60)), Some(true));
        assert!(m.is_locked());
    }

    #[test]
    fn the_first_background_instant_counts() {
        let t0 = Instant::now();
        let mut m = LockMachine::new(true, 30);
        m.unlock();
        m.background(t0);
        m.background(t0 + Duration::from_secs(20));
        // The first background instant counts, so 35 s after t0 locks.
        assert_eq!(m.foreground(t0 + Duration::from_secs(35)), Some(true));
    }

    #[test]
    fn zero_seconds_locks_on_every_return() {
        let t0 = Instant::now();
        let mut m = LockMachine::new(true, 0);
        m.unlock();
        m.background(t0);
        assert_eq!(m.foreground(t0), Some(true));
    }

    #[test]
    fn disabled_lock_never_locks_and_disabling_unlocks() {
        let t0 = Instant::now();
        let mut m = LockMachine::new(false, 0);
        m.background(t0);
        assert_eq!(m.foreground(t0 + Duration::from_secs(999)), None);
        assert!(!m.is_locked());

        let mut locked = LockMachine::new(true, 60);
        locked.configure(false, 60);
        assert!(!locked.is_locked());
    }

    #[test]
    fn enabling_does_not_lock_the_running_app() {
        let mut m = LockMachine::new(false, 60);
        m.configure(true, 60);
        assert!(!m.is_locked());
    }

    #[test]
    fn external_picker_does_not_count_as_background() {
        let t0 = Instant::now();
        let mut m = LockMachine::new(true, 10);
        m.unlock();
        m.set_external_ui(true);
        m.background(t0);
        m.set_external_ui(false);
        assert_eq!(m.foreground(t0 + Duration::from_secs(600)), None);
        assert!(!m.is_locked());
    }

    #[test]
    fn unlock_reports_change_once() {
        let mut m = LockMachine::new(true, 60);
        assert!(m.unlock());
        assert!(!m.unlock());
    }
}
