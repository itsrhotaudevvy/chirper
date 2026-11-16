//! Linux. The unread count goes to the Unity launcher over D-Bus (best effort: desktops without it
//! simply show no count). There is no system biometric API, so the app lock is unavailable, and
//! there is no portable screen-capture exclusion for X11 or Wayland. Haptics are a no-op.

use std::collections::HashMap;

use tauri::AppHandle;
use zbus::blocking::Connection;
use zbus::zvariant::Value;

use super::{HapticKind, unsupported};
use crate::error::CmdResult;

const LAUNCHER_PATH: &str = "/com/canonical/unity/launcherentry/chirper";
const LAUNCHER_INTERFACE: &str = "com.canonical.Unity.LauncherEntry";
/// The launcher matches the application by its desktop file, which tauri-bundler names after the
/// productName (`Chirper.desktop`, see tauri-bundler src/bundle/linux/freedesktop/mod.rs).
const APPLICATION_URI: &str = "application://Chirper.desktop";

/// No system authentication is offered here, so the app lock stays off.
pub fn app_lock_supported() -> bool {
    false
}

/// Haptics do not exist on desktop Linux; the contract makes them a no-op.
pub fn haptic(_app: &AppHandle, _kind: HapticKind) -> CmdResult<()> {
    Ok(())
}

/// Publishes the count to the Unity launcher. Failures are logged at debug level only: the badge
/// is cosmetic and desktops without a launcher listener never answer.
pub fn set_badge(_app: &AppHandle, count: i64) -> CmdResult<()> {
    if let Err(e) = publish_count(count) {
        log::debug!("no launcher badge: {e}");
    }
    Ok(())
}

fn publish_count(count: i64) -> zbus::Result<()> {
    let connection = Connection::session()?;
    let properties: HashMap<&str, Value<'_>> = HashMap::from([
        ("count", Value::from(count)),
        ("count-visible", Value::from(count > 0)),
    ]);
    // A broadcast: the signal has no destination bus name.
    connection.emit_signal(
        None::<&str>,
        LAUNCHER_PATH,
        LAUNCHER_INTERFACE,
        "Update",
        &(APPLICATION_URI, properties),
    )
}

/// X11 and Wayland have no portable way to exclude a window from capture.
pub fn set_secure_screen(_app: &AppHandle, _enabled: bool) -> CmdResult<()> {
    unsupported("screen privacy")
}

/// The title bar is drawn by the desktop, so there is nothing to style.
pub fn style_titlebar(_window: &tauri::WebviewWindow) {}
