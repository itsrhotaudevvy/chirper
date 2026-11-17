//! App preferences in `<appData>/prefs.json` (mode 0600). Rust owns the notification and lock
//! decisions, so they keep working while the webview is hidden.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const PREFS_FILE: &str = "prefs.json";
const MAX_LOCK_AFTER_SECS: u64 = 86_400;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct Prefs {
    pub notifications: bool,
    pub hide_notification_content: bool,
    pub app_lock: bool,
    pub lock_after_secs: u64,
    pub secure_screen: bool,
    pub close_to_tray: bool,
    pub launch_at_login: bool,
    pub haptics: bool,
    /// Network ids (for example "whatsapp") whose messages never raise a notification.
    pub muted_networks: Vec<String>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            notifications: true,
            hide_notification_content: false,
            app_lock: false,
            lock_after_secs: 60,
            // Mobile screens hold chat content, so they start protected. Desktop starts open.
            secure_screen: cfg!(any(target_os = "android", target_os = "ios")),
            close_to_tray: true,
            launch_at_login: false,
            haptics: true,
            muted_networks: Vec::new(),
        }
    }
}

/// Whether messages of `network` are kept from notifications by the user's choice.
pub fn network_muted(prefs: &Prefs, network: &str) -> bool {
    prefs.muted_networks.iter().any(|muted| muted == network)
}

impl Prefs {
    /// Rejects values the shell cannot act on.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.lock_after_secs > MAX_LOCK_AFTER_SECS {
            return Err("lockAfterSecs");
        }
        Ok(())
    }

    /// Applies a partial update: only keys present in `patch` change. Unknown keys and wrong types
    /// are rejected, and the result is validated.
    pub fn merged(&self, patch: &serde_json::Value) -> Result<Self, &'static str> {
        let serde_json::Value::Object(patch) = patch else {
            return Err("prefs");
        };
        let mut current = serde_json::to_value(self).map_err(|_| "prefs")?;
        let object = current.as_object_mut().ok_or("prefs")?;
        for (key, value) in patch {
            if !object.contains_key(key) {
                return Err("prefs");
            }
            object.insert(key.clone(), value.clone());
        }
        let next: Self = serde_json::from_value(current).map_err(|_| "prefs")?;
        next.validate()?;
        Ok(next)
    }
}

/// Reads the preferences. A missing or unreadable file gives the defaults; an unreadable file is
/// logged and replaced on the next save.
pub fn load(path: &Path) -> Prefs {
    match fs::read(path) {
        Ok(bytes) => match serde_json::from_slice::<Prefs>(&bytes) {
            Ok(prefs) if prefs.validate().is_ok() => prefs,
            _ => {
                log::warn!("prefs.json is invalid; using defaults");
                Prefs::default()
            }
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Prefs::default(),
        Err(e) => {
            log::warn!("cannot read prefs.json: {}", e.kind());
            Prefs::default()
        }
    }
}

/// Writes the preferences atomically: a temporary file with mode 0600, then a rename.
pub fn save(path: &Path, prefs: &Prefs) -> io::Result<()> {
    let text = serde_json::to_vec_pretty(prefs).map_err(io::Error::other)?;
    let tmp: PathBuf = path.with_extension("json.tmp");
    let mut file = create_private(&tmp)?;
    file.write_all(&text)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, path)
}

#[cfg(unix)]
fn create_private(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_private(path: &Path) -> io::Result<fs::File> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("chirper-prefs-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir.join(PREFS_FILE)
    }

    #[test]
    fn missing_file_gives_defaults() {
        let path = temp_path("missing");
        let _ = fs::remove_file(&path);
        assert_eq!(load(&path), Prefs::default());
    }

    #[test]
    fn save_then_load_round_trips_and_is_private() {
        let path = temp_path("round");
        let prefs = Prefs {
            lock_after_secs: 300,
            haptics: false,
            ..Prefs::default()
        };
        save(&path, &prefs).unwrap();
        assert_eq!(load(&path), prefs);
        assert!(!path.with_extension("json.tmp").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn corrupt_file_gives_defaults() {
        let path = temp_path("corrupt");
        fs::write(&path, b"{not json").unwrap();
        assert_eq!(load(&path), Prefs::default());
    }

    #[test]
    fn merge_changes_only_named_keys() {
        let base = Prefs::default();
        let next = base
            .merged(&serde_json::json!({ "closeToTray": false }))
            .unwrap();
        assert!(!next.close_to_tray);
        assert_eq!(next.lock_after_secs, base.lock_after_secs);
    }

    #[test]
    fn merge_rejects_unknown_keys_bad_types_and_out_of_range_values() {
        let base = Prefs::default();
        assert!(base.merged(&serde_json::json!({ "bogus": 1 })).is_err());
        assert!(
            base.merged(&serde_json::json!({ "haptics": "yes" }))
                .is_err()
        );
        assert!(
            base.merged(&serde_json::json!({ "lockAfterSecs": 999_999 }))
                .is_err()
        );
        assert!(base.merged(&serde_json::json!([1, 2])).is_err());
    }
}
