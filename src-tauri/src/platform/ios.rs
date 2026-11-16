//! iOS. Haptics and authentication use the mobile plugins. The chirper-native plugin provides the
//! Keychain key (`ThisDeviceOnly`), the secure-screen cover for the app switcher and screen
//! recording, the app badge and the form factor. The plugin also registers the background refresh.

use tauri::AppHandle;
use tauri_plugin_chirper_native::ChirperNativeExt;
use zeroize::Zeroizing;

use super::mobile;
use crate::error::{CmdError, CmdResult, code};

/// Face ID and Touch ID prompts work on iOS through the plugin.
pub const APP_LOCK_SUPPORTED: bool = true;

pub use mobile::{authenticate, haptic};

/// The database key in the Keychain, created on first use.
#[cfg(not(feature = "mock"))]
pub fn load_db_key(app: &AppHandle) -> Result<Option<Zeroizing<[u8; 32]>>, String> {
    tauri_plugin_chirper_native::load_or_create_db_key(app)
        .map(Some)
        .map_err(|e| e.to_string())
}

/// The mock core keeps nothing on disk, so the mock build creates no key.
#[cfg(feature = "mock")]
pub fn load_db_key(_app: &AppHandle) -> Result<Option<Zeroizing<[u8; 32]>>, String> {
    Ok(None)
}

/// The app icon badge, set when the user allows badges.
pub fn set_badge(app: &AppHandle, count: i64) -> CmdResult<()> {
    app.chirper_native().set_badge(count).map_err(native_error)
}

/// The cover over the window while the app is in the app switcher or the screen is recorded.
pub fn set_secure_screen(app: &AppHandle, enabled: bool) -> CmdResult<()> {
    app.chirper_native()
        .set_secure_screen(enabled)
        .map_err(native_error)
}

fn native_error(err: tauri_plugin_chirper_native::Error) -> CmdError {
    CmdError::new(code::CORE, err.to_string())
}
