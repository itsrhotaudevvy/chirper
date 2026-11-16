//! Declares the app commands so that each one gets an `allow-<command>` permission that the
//! capability in `capabilities/default.json` grants to the main window.

/// Every `#[tauri::command]` of `src/commands.rs` that the webview may call. Keep in sync with
/// the handler list in `src/lib.rs`.
const COMMANDS: &[&str] = &[
    "start",
    "networks",
    "accounts",
    "start_login",
    "submit_login_input",
    "submit_login_cookies",
    "wait_login_step",
    "cancel_login",
    "logout",
    "list_chats",
    "get_chat",
    "list_messages",
    "load_older",
    "sync_history",
    "mark_unread",
    "send_text",
    "send_media",
    "edit",
    "redact",
    "react",
    "unreact",
    "mark_read",
    "set_typing",
    "set_chat_flags",
    "media_file",
    "search",
    "settings",
    "set_settings",
    "app_info",
    "haptic",
    "set_badge",
    "notify_message",
    "lock_status",
    "unlock",
    "set_secure_screen",
    "open_external",
    "set_app_prefs",
    "get_app_prefs",
    "pick_files",
];

fn main() {
    let attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS));
    tauri_build::try_build(attributes).expect("tauri build script failed");
}
