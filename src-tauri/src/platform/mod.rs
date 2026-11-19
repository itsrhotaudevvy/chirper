//! Platform integration used by the commands. The per-OS files do the platform work or return
//! `unsupported`; nothing pretends. Mobile haptics and authentication go through plugins in
//! `mobile.rs`.

use serde::Deserialize;
use tauri::{AppHandle, Manager, WebviewWindow};

use crate::error::{CmdError, CmdResult, code};

#[cfg(any(target_os = "android", target_os = "ios"))]
mod mobile;

#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod imp;
#[cfg(target_os = "windows")]
#[path = "windows.rs"]
mod imp;
#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod imp;
#[cfg(target_os = "android")]
#[path = "android.rs"]
mod imp;
#[cfg(target_os = "ios")]
#[path = "ios.rs"]
mod imp;

pub use imp::{authenticate, haptic, set_badge, set_secure_screen};

/// Whether the app lock can authenticate on this device. Desktop asks the OS at runtime (Touch ID
/// or the device password on macOS, Windows Hello on Windows); mobile is fixed.
#[cfg(any(target_os = "android", target_os = "ios"))]
pub fn app_lock_supported() -> bool {
    imp::APP_LOCK_SUPPORTED
}

/// Whether the app lock can authenticate on this device (see the mobile variant above).
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn app_lock_supported() -> bool {
    imp::app_lock_supported()
}

/// Background colours of the `--bg` token in `ui/src/styles/tokens.css`, per theme. They are set
/// before the first paint so the window never flashes the wrong colour.
#[cfg(desktop)]
const DARK_BACKGROUND: tauri::window::Color = tauri::window::Color(0x0E, 0x15, 0x17, 0xFF);
#[cfg(desktop)]
const LIGHT_BACKGROUND: tauri::window::Color = tauri::window::Color(0xF6, 0xF8, 0xF9, 0xFF);

/// Sets the main window's background to the system theme's `--bg` and styles its titlebar. Call
/// once from `setup`. Failures are logged; the window works without them.
#[cfg(desktop)]
pub fn prepare_main_window(app: &AppHandle) {
    let window = match main_window(app) {
        Ok(window) => window,
        Err(e) => {
            log::warn!("{}", e.message);
            return;
        }
    };
    let dark = matches!(window.theme(), Ok(tauri::Theme::Dark));
    let background = if dark {
        DARK_BACKGROUND
    } else {
        LIGHT_BACKGROUND
    };
    if let Err(e) = window.set_background_color(Some(background)) {
        log::warn!("cannot set the window background: {e}");
    }
    imp::style_titlebar(&window);
}

/// Haptic kinds of CONTRACT.md section C.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HapticKind {
    Selection,
    Light,
    Medium,
    Heavy,
    Success,
    Warning,
    Error,
}

/// The `formFactor` of `app_info`: `phone` or `tablet` on mobile, `desktop` elsewhere. On mobile
/// the native side answers, and it blocks, so callers should not be on the main thread.
#[cfg(any(target_os = "android", target_os = "ios"))]
pub fn form_factor(app: &AppHandle) -> &'static str {
    use tauri_plugin_chirper_native::{ChirperNativeExt, FormFactor};
    match app.chirper_native().form_factor() {
        Ok(FormFactor::Tablet) => "tablet",
        Ok(FormFactor::Phone) => "phone",
        Err(e) => {
            log::warn!("cannot read the form factor: {e}");
            "phone"
        }
    }
}

/// The `formFactor` of `app_info` on desktop.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn form_factor(_app: &AppHandle) -> &'static str {
    "desktop"
}

/// The database key from the platform store. Android and iOS keep it through the chirper-native
/// plugin (see `android.rs` and `ios.rs`). The mock core keeps nothing on disk, so it needs no key.
#[cfg(all(
    not(any(target_os = "android", target_os = "ios")),
    not(feature = "mock")
))]
pub fn load_db_key(_app: &AppHandle) -> Result<Option<zeroize::Zeroizing<[u8; 32]>>, String> {
    crate::keystore::load_or_create().map(Some)
}

#[cfg(all(not(any(target_os = "android", target_os = "ios")), feature = "mock"))]
pub fn load_db_key(_app: &AppHandle) -> Result<Option<zeroize::Zeroizing<[u8; 32]>>, String> {
    Ok(None)
}

#[cfg(any(target_os = "android", target_os = "ios"))]
pub use imp::load_db_key;

/// Keeps the app running in the background while accounts are connected. Android runs a
/// foreground service for this. iOS has no such service: it relies on the background refresh.
#[cfg(target_os = "android")]
pub use imp::set_background_service;

#[cfg(not(target_os = "android"))]
pub fn set_background_service(_app: &AppHandle, _running: bool) -> CmdResult<()> {
    Ok(())
}

/// Shows the notification of a new message. Android posts a messaging notification with an inline
/// reply; the other platforms use the notification plugin.
#[cfg(target_os = "android")]
pub use imp::show_message;

#[cfg(not(target_os = "android"))]
pub fn show_message(
    app: &AppHandle,
    _room_id: &str,
    title: &str,
    body: &str,
    _hide_content: bool,
) -> CmdResult<()> {
    use tauri_plugin_notification::NotificationExt;
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|e| CmdError::new(code::CORE, e.to_string()))
}

/// Connects the inline replies of Android notifications to the webview. Nothing to connect elsewhere.
#[cfg(target_os = "android")]
pub use imp::connect_replies;

#[cfg(not(target_os = "android"))]
pub fn connect_replies(_app: &AppHandle) -> CmdResult<()> {
    Ok(())
}

#[cfg(target_os = "android")]
pub use imp::open_login_webview;

#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub async fn open_login_webview(
    _app: &AppHandle,
    _url: &str,
    _extract_js: &str,
    _wait_for_pattern: &str,
    _user_agent: &str,
    _title: &str,
) -> CmdResult<crate::commands::LoginWebviewResult> {
    unsupported("login webview")
}

/// The local path of a file chosen in the picker. Android copies a `content://` file into the app
/// cache, because the core reads files by path.
#[cfg(target_os = "android")]
pub use imp::picked_path;

#[cfg(not(target_os = "android"))]
pub fn picked_path(
    _app: &AppHandle,
    entry: tauri_plugin_dialog::FilePath,
) -> CmdResult<std::path::PathBuf> {
    entry
        .into_path()
        .map_err(|_| CmdError::unsupported("this file location"))
}

/// The main window, or an error when it does not exist.
pub fn main_window(app: &AppHandle) -> CmdResult<WebviewWindow> {
    app.get_webview_window("main")
        .ok_or_else(|| CmdError::new(code::CORE, "the main window is missing"))
}

/// The error for a feature that is not implemented on this platform yet.
pub fn unsupported<T>(feature: &str) -> CmdResult<T> {
    Err(CmdError::unsupported(feature))
}

/// Drivers this platform provides to the core because they need the OS (the Android SMS provider).
/// Desktop and iOS provide none; the core then lists those networks with their reason.
pub fn host_networks(_app: &AppHandle) -> chirper_core::HostNetworks {
    chirper_core::HostNetworks::default()
}
