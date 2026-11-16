//! The commands of CONTRACT.md section C. Core commands mirror the typed core methods; they fail
//! with `locked` while the app is locked and `not_ready` until the core has opened.

use std::collections::BTreeMap;
use std::path::Path;

use chirper_core::api::MediaUpload;
use chirper_core::model::{
    Account, Chat, ChatFilter, ChatFlags, CoreSettings, LoginState, LoginStepType, Message, Network,
};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Manager, State, Url};
use tauri_plugin_dialog::{DialogExt, FilePath};
use tauri_plugin_opener::OpenerExt;

use crate::error::{CmdError, CmdResult, code};
use crate::lock;
use crate::platform::{self, HapticKind};
use crate::prefs::Prefs;
use crate::startup;
use crate::state::AppState;

const MAX_URL_BYTES: usize = 2048;
const MEDIA_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "heic", "avif", "mp4", "mov", "webm", "mp3", "m4a", "ogg",
    "opus", "wav", "flac",
];

// ---- result shapes --------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HasMore {
    pub has_more: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventIdOut {
    pub event_id: String,
}

#[derive(Serialize)]
pub struct PathOut {
    pub path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: &'static str,
    pub platform: &'static str,
    pub form_factor: &'static str,
}

#[derive(Serialize)]
pub struct Unlocked {
    pub unlocked: bool,
}

#[derive(Serialize)]
pub struct LockStatus {
    pub locked: bool,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct PickedFile {
    pub path: String,
    pub name: String,
    pub mime: String,
    pub size: u64,
}

// ---- helpers --------------------------------------------------------------------------------

fn event_out(id: String) -> EventIdOut {
    EventIdOut { event_id: id }
}

/// Maps a mime guess from the file extension. Anything unknown is `application/octet-stream`.
pub fn guess_mime(name: &str) -> &'static str {
    let extension = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match extension.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "avif" => "image/avif",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "ogg" | "opus" => "audio/ogg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

/// The file name for the notification text, or the app name when content is hidden.
fn notification_text(prefs: &Prefs, title: &str, body: &str) -> (String, String) {
    if prefs.hide_notification_content {
        ("Chirper".to_owned(), "New message".to_owned())
    } else {
        (title.to_owned(), body.to_owned())
    }
}

fn check_http_url(url: &str) -> CmdResult<Url> {
    if url.len() > MAX_URL_BYTES {
        return Err(CmdError::invalid("url"));
    }
    let parsed = Url::parse(url).map_err(|_| CmdError::invalid("url"))?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed),
        _ => Err(CmdError::invalid("url")),
    }
}

fn set_lock_config(state: &AppState, prefs: &Prefs) {
    state
        .lock_machine()
        .configure(prefs.app_lock, prefs.lock_after_secs);
}

// ---- core: accounts and login ---------------------------------------------------------------

async fn get_core(state: &AppState) -> CmdResult<chirper_core::Core> {
    if let Ok(c) = state.ready_core() {
        return Ok(c);
    }
    state.ensure_unlocked()?;
    for _ in 0..50 {
        tauri::async_runtime::spawn_blocking(|| {
            std::thread::sleep(std::time::Duration::from_millis(100))
        })
        .await
        .ok();
        if let Ok(c) = state.ready_core() {
            return Ok(c);
        }
    }
    state.ready_core()
}

#[tauri::command]
pub async fn start(state: State<'_, AppState>) -> CmdResult<()> {
    Ok(get_core(&state).await?.start().await?)
}

#[tauri::command]
pub async fn networks(state: State<'_, AppState>) -> CmdResult<Vec<Network>> {
    Ok(get_core(&state).await?.networks().await?)
}

#[tauri::command]
pub async fn accounts(state: State<'_, AppState>) -> CmdResult<Vec<Account>> {
    Ok(get_core(&state).await?.accounts().await?)
}

#[tauri::command]
pub async fn start_login(
    state: State<'_, AppState>,
    network: String,
    flow_id: String,
) -> CmdResult<LoginState> {
    Ok(state.ready_core()?.start_login(&network, &flow_id).await?)
}

#[tauri::command]
pub async fn submit_login_input(
    state: State<'_, AppState>,
    process_id: String,
    values: BTreeMap<String, String>,
) -> CmdResult<LoginState> {
    Ok(state
        .ready_core()?
        .submit_login_input(&process_id, &values)
        .await?)
}

#[tauri::command]
pub async fn submit_login_cookies(
    state: State<'_, AppState>,
    process_id: String,
    cookies: BTreeMap<String, String>,
) -> CmdResult<LoginState> {
    Ok(state
        .ready_core()?
        .submit_login_cookies(&process_id, &cookies)
        .await?)
}

#[tauri::command]
pub async fn wait_login_step(
    app: AppHandle,
    state: State<'_, AppState>,
    process_id: String,
) -> CmdResult<LoginState> {
    let core = state.ready_core()?;
    let login = core.wait_login_step(&process_id).await?;
    if login.step.kind == LoginStepType::Complete {
        startup::sync_background_service(&app, &core).await;
    }
    Ok(login)
}

#[tauri::command]
pub async fn cancel_login(state: State<'_, AppState>, process_id: String) -> CmdResult<()> {
    Ok(state.ready_core()?.cancel_login(&process_id).await?)
}

#[tauri::command]
pub async fn logout(
    app: AppHandle,
    state: State<'_, AppState>,
    network: String,
    login_id: String,
) -> CmdResult<()> {
    let core = state.ready_core()?;
    core.logout(&network, &login_id).await?;
    startup::sync_background_service(&app, &core).await;
    Ok(())
}

// ---- core: chats and messages ---------------------------------------------------------------

#[tauri::command]
pub async fn list_chats(state: State<'_, AppState>, filter: ChatFilter) -> CmdResult<Vec<Chat>> {
    Ok(get_core(&state).await?.list_chats(&filter).await?)
}

#[tauri::command]
pub async fn get_chat(state: State<'_, AppState>, room_id: String) -> CmdResult<Option<Chat>> {
    Ok(get_core(&state).await?.get_chat(&room_id).await?)
}

#[tauri::command]
pub async fn list_messages(
    state: State<'_, AppState>,
    room_id: String,
    before_order: i64,
    limit: u32,
) -> CmdResult<Vec<Message>> {
    Ok(get_core(&state)
        .await?
        .list_messages(&room_id, before_order, limit)
        .await?)
}

#[tauri::command]
pub async fn load_older(state: State<'_, AppState>, room_id: String) -> CmdResult<HasMore> {
    let has_more = get_core(&state).await?.load_older(&room_id).await?;
    Ok(HasMore { has_more })
}

#[tauri::command]
pub async fn sync_history(state: State<'_, AppState>, network: String) -> CmdResult<()> {
    Ok(get_core(&state).await?.sync_history(&network).await?)
}

#[tauri::command]
pub async fn mark_unread(
    state: State<'_, AppState>,
    room_id: String,
    unread: bool,
) -> CmdResult<()> {
    Ok(state.ready_core()?.mark_unread(&room_id, unread).await?)
}

#[tauri::command]
pub async fn send_text(
    state: State<'_, AppState>,
    room_id: String,
    text: String,
    reply_to: Option<String>,
    txn_id: String,
) -> CmdResult<EventIdOut> {
    let id = state
        .ready_core()?
        .send_text(&room_id, &text, reply_to.as_deref(), &txn_id)
        .await?;
    Ok(event_out(id))
}

/// Sends a file chosen with `pick_files`. Any other path is refused.
/// The arguments mirror CONTRACT.md section B, so the list is long.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn send_media(
    state: State<'_, AppState>,
    room_id: String,
    path: String,
    mime: String,
    file_name: String,
    caption: Option<String>,
    kind: String,
    reply_to: Option<String>,
    txn_id: String,
) -> CmdResult<EventIdOut> {
    let core = state.ready_core()?;
    let canonical = std::fs::canonicalize(&path).map_err(|_| CmdError::invalid("path"))?;
    if !state.is_upload_allowed(&canonical) {
        return Err(CmdError::invalid("path"));
    }
    let caption = caption.unwrap_or_default();
    let upload = MediaUpload {
        path: &canonical,
        mime_type: &mime,
        file_name: &file_name,
        caption: &caption,
        kind: &kind,
        reply_to: reply_to.as_deref(),
        txn_id: &txn_id,
    };
    let id = core.send_media(&room_id, upload).await?;
    Ok(event_out(id))
}

#[tauri::command]
pub async fn edit(
    state: State<'_, AppState>,
    room_id: String,
    event_id: String,
    text: String,
) -> CmdResult<()> {
    Ok(state.ready_core()?.edit(&room_id, &event_id, &text).await?)
}

#[tauri::command]
pub async fn redact(
    state: State<'_, AppState>,
    room_id: String,
    event_id: String,
) -> CmdResult<()> {
    Ok(state.ready_core()?.redact(&room_id, &event_id).await?)
}

#[tauri::command]
pub async fn react(
    state: State<'_, AppState>,
    room_id: String,
    event_id: String,
    key: String,
) -> CmdResult<()> {
    Ok(state.ready_core()?.react(&room_id, &event_id, &key).await?)
}

#[tauri::command]
pub async fn unreact(
    state: State<'_, AppState>,
    room_id: String,
    event_id: String,
    key: String,
) -> CmdResult<()> {
    Ok(state
        .ready_core()?
        .unreact(&room_id, &event_id, &key)
        .await?)
}

#[tauri::command]
pub async fn mark_read(
    state: State<'_, AppState>,
    room_id: String,
    event_id: String,
) -> CmdResult<()> {
    Ok(state.ready_core()?.mark_read(&room_id, &event_id).await?)
}

#[tauri::command]
pub async fn set_typing(
    state: State<'_, AppState>,
    room_id: String,
    typing: bool,
) -> CmdResult<()> {
    Ok(state.ready_core()?.set_typing(&room_id, typing).await?)
}

#[tauri::command]
pub async fn set_chat_flags(
    state: State<'_, AppState>,
    room_id: String,
    flags: ChatFlags,
) -> CmdResult<()> {
    Ok(state.ready_core()?.set_chat_flags(&room_id, &flags).await?)
}

#[tauri::command]
pub async fn media_file(state: State<'_, AppState>, url: String) -> CmdResult<PathOut> {
    let path = get_core(&state).await?.media_file(&url).await?;
    let path = path
        .to_str()
        .ok_or_else(|| CmdError::new(code::CORE, "the media path is not UTF-8"))?
        .to_owned();
    Ok(PathOut { path })
}

#[tauri::command]
pub async fn search(
    state: State<'_, AppState>,
    query: String,
    limit: u32,
) -> CmdResult<Vec<Message>> {
    Ok(get_core(&state).await?.search(&query, limit).await?)
}

#[tauri::command]
pub async fn settings(state: State<'_, AppState>) -> CmdResult<CoreSettings> {
    Ok(state.ready_core()?.settings().await?)
}

#[tauri::command]
pub async fn set_settings(state: State<'_, AppState>, settings: CoreSettings) -> CmdResult<()> {
    Ok(state.ready_core()?.set_settings(&settings).await?)
}

// ---- app: information, feedback and notifications ------------------------------------------

/// Async so that the mobile form-factor query never blocks the main thread.
#[tauri::command]
pub async fn app_info(app: AppHandle) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        platform: tauri_plugin_os::platform(),
        form_factor: platform::form_factor(&app),
    }
}

#[tauri::command]
pub fn haptic(app: AppHandle, state: State<'_, AppState>, kind: HapticKind) -> CmdResult<()> {
    state.ensure_unlocked()?;
    if !state.prefs().haptics {
        return Ok(());
    }
    platform::haptic(&app, kind)
}

#[tauri::command]
pub fn set_badge(app: AppHandle, state: State<'_, AppState>, count: i64) -> CmdResult<()> {
    state.ensure_unlocked()?;
    if count < 0 {
        return Err(CmdError::invalid("count"));
    }
    platform::set_badge(&app, count)?;
    #[cfg(desktop)]
    crate::tray::set_unread(&app, count > 0)?;
    Ok(())
}

/// Posts a notification for `roomId`. The avatar path is accepted but not drawn yet (W3a/W3b).
#[tauri::command]
pub fn notify_message(
    app: AppHandle,
    state: State<'_, AppState>,
    room_id: String,
    title: String,
    body: String,
    avatar_path: Option<String>,
) -> CmdResult<()> {
    state.ensure_unlocked()?;
    chirper_core::validate::room_id(&room_id)?;
    // Avatars are not drawn in notifications yet; the argument is accepted for the contract.
    let _ = avatar_path;
    let prefs = state.prefs();
    if !prefs.notifications {
        return Ok(());
    }
    let (title, body) = notification_text(&prefs, &title, &body);
    platform::show_message(
        &app,
        &room_id,
        &title,
        &body,
        prefs.hide_notification_content,
    )
}

// ---- app: lock, privacy and links -----------------------------------------------------------

#[tauri::command]
pub fn lock_status(app: AppHandle) -> LockStatus {
    LockStatus {
        locked: lock::is_locked(&app),
    }
}

#[tauri::command]
pub async fn unlock(app: AppHandle) -> CmdResult<Unlocked> {
    let unlocked = lock::unlock(&app).await?;
    Ok(Unlocked { unlocked })
}

#[tauri::command]
pub fn set_secure_screen(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> CmdResult<()> {
    state.ensure_unlocked()?;
    platform::set_secure_screen(&app, enabled)?;
    let next = Prefs {
        secure_screen: enabled,
        ..state.prefs()
    };
    state
        .store_prefs(next)
        .map_err(|e| CmdError::new(code::IO, e.to_string()))
}

#[tauri::command]
pub fn open_external(app: AppHandle, state: State<'_, AppState>, url: String) -> CmdResult<()> {
    state.ensure_unlocked()?;
    let parsed = check_http_url(&url)?;
    app.opener()
        .open_url(parsed.as_str(), None::<&str>)
        .map_err(|e| CmdError::new(code::CORE, e.to_string()))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginWebviewArgs {
    pub url: String,
    pub extract_js: Option<String>,
    #[allow(dead_code)]
    pub wait_for_pattern: Option<String>,
    pub user_agent: Option<String>,
    pub title: Option<String>,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LoginWebviewResult {
    pub token: Option<String>,
    pub cancelled: bool,
}

#[tauri::command]
pub async fn open_login_webview(
    app: AppHandle,
    state: State<'_, AppState>,
    url: String,
    extract_js: Option<String>,
    wait_for_pattern: Option<String>,
    user_agent: Option<String>,
    title: Option<String>,
) -> CmdResult<LoginWebviewResult> {
    state.ensure_unlocked()?;
    let args = LoginWebviewArgs {
        url,
        extract_js,
        wait_for_pattern,
        user_agent,
        title,
    };
    #[cfg(desktop)]
    {
        run_desktop_login_webview(app, args).await
    }
    #[cfg(mobile)]
    {
        platform::open_login_webview(
            &app,
            &args.url,
            args.extract_js.as_deref().unwrap_or(""),
            args.wait_for_pattern.as_deref().unwrap_or(""),
            args.user_agent.as_deref().unwrap_or(""),
            args.title.as_deref().unwrap_or(""),
        )
        .await
    }
}

#[cfg(desktop)]
static LOGIN_WINDOW_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[cfg(desktop)]
async fn run_desktop_login_webview(
    app: AppHandle,
    args: LoginWebviewArgs,
) -> CmdResult<LoginWebviewResult> {
    let parsed_url = check_http_url(&args.url)?;
    let title = args.title.unwrap_or_else(|| "Sign In".to_string());
    let seq = LOGIN_WINDOW_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let window_label = format!("login-webview-{seq}");

    let (tx, mut rx) = tauri::async_runtime::channel::<Option<String>>(1);
    let tx_nav = tx.clone();
    let app_handle = app.clone();
    let label_close = window_label.clone();

    let custom_js = args.extract_js.as_deref().unwrap_or("");
    let custom_inject = if !custom_js.is_empty() {
        format!("try {{ var c = {custom_js}; if (c && typeof c === 'string' && c.length > 20) return c; }} catch(e) {{}}")
    } else {
        String::new()
    };

    let injection_script = format!(
        r#"
        (function() {{
            if (window.__chirper_login_injected) return;
            window.__chirper_login_injected = true;
            function tryExtract() {{
                {custom_inject}
                try {{
                    var raw = localStorage.getItem('token');
                    if (raw) {{
                        var v = raw;
                        try {{ v = JSON.parse(raw); }} catch(e) {{}}
                        if (typeof v === 'string' && v.length > 20) return v;
                    }}
                }} catch(e) {{}}
                try {{
                    var m = [];
                    window.webpackChunkdiscord_app?.push([['chirper_' + Date.now()], {{}}, function(e) {{
                        for (var c in e.c) m.push(e.c[c]);
                    }}]);
                    for (var i = 0; i < m.length; i++) {{
                        var mod = m[i]?.exports?.default;
                        if (mod && typeof mod.getToken === 'function') {{
                            var t = mod.getToken();
                            if (typeof t === 'string' && t.length > 20) return t;
                        }}
                    }}
                }} catch(e) {{}}
                return null;
            }}
            var interval = setInterval(function() {{
                var tok = tryExtract();
                if (tok) {{
                    clearInterval(interval);
                    window.location.href = 'https://chirper-login.local/?token=' + encodeURIComponent(tok);
                }}
            }}, 500);
        }})();
        "#
    );

    let default_ua = match () {
        #[cfg(target_os = "macos")]
        () => "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        #[cfg(target_os = "windows")]
        () => "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        #[cfg(target_os = "linux")]
        () => "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        () => "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
    };
    let user_agent = args.user_agent.as_deref().unwrap_or(default_ua);

    let mut builder = tauri::WebviewWindowBuilder::new(
        &app,
        &window_label,
        tauri::WebviewUrl::External(parsed_url),
    )
    .title(&title)
    .inner_size(840.0, 720.0)
    .center()
    .user_agent(user_agent)
    .initialization_script(&injection_script);

    builder = builder.on_navigation(move |nav_url| {
        if nav_url.host_str() == Some("chirper-login.local") {
            let token = nav_url
                .query_pairs()
                .find(|(k, _)| k == "token")
                .map(|(_, v)| v.into_owned());
            if let Some(t) = token {
                let tx = tx_nav.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = tx.send(Some(t)).await;
                });
                if let Some(w) = app_handle.get_webview_window(&label_close) {
                    let _ = w.close();
                }
                return false;
            }
        }
        true
    });

    let window = builder.build().map_err(|e| CmdError::new(code::CORE, e.to_string()))?;

    let tx_close = tx.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::Destroyed = event {
            let tx = tx_close.clone();
            tauri::async_runtime::spawn(async move {
                let _ = tx.send(None).await;
            });
        }
    });

    let token_opt = rx.recv().await.flatten();
    Ok(LoginWebviewResult {
        cancelled: token_opt.is_none(),
        token: token_opt,
    })
}

/// Applies a partial update of the preferences. Side effects run first, so a refused change (for
/// example app lock on a platform that cannot authenticate) saves nothing.
#[tauri::command]
pub fn set_app_prefs(app: AppHandle, state: State<'_, AppState>, prefs: Value) -> CmdResult<Prefs> {
    state.ensure_unlocked()?;
    let current = state.prefs();
    let next = current.merged(&prefs).map_err(CmdError::invalid)?;
    if next.app_lock && !platform::app_lock_supported() && !current.app_lock {
        return platform::unsupported("app lock");
    }
    if next.secure_screen != current.secure_screen {
        match platform::set_secure_screen(&app, next.secure_screen) {
            // Turning protection off on a platform that never enforced it changes nothing.
            Err(e) if e.code == code::UNSUPPORTED && !next.secure_screen => {}
            result => result?,
        }
    }
    if next.launch_at_login != current.launch_at_login {
        set_launch_at_login(&app, next.launch_at_login)?;
    }
    state
        .store_prefs(next.clone())
        .map_err(|e| CmdError::new(code::IO, e.to_string()))?;
    set_lock_config(&state, &next);
    Ok(next)
}

#[cfg(desktop)]
fn set_launch_at_login(app: &AppHandle, enabled: bool) -> CmdResult<()> {
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    result.map_err(|e| CmdError::new(code::IO, e.to_string()))
}

#[cfg(mobile)]
fn set_launch_at_login(_app: &AppHandle, _enabled: bool) -> CmdResult<()> {
    platform::unsupported("launch at login")
}

#[tauri::command]
pub fn get_app_prefs(state: State<'_, AppState>) -> Prefs {
    state.prefs()
}

// ---- files ----------------------------------------------------------------------------------

/// Opens the native file dialog. The returned paths are the only ones `send_media` accepts.
#[tauri::command]
pub async fn pick_files(
    app: AppHandle,
    state: State<'_, AppState>,
    kind: String,
) -> CmdResult<Vec<PickedFile>> {
    state.ensure_unlocked()?;
    let media = match kind.as_str() {
        "media" => true,
        "file" => false,
        _ => return Err(CmdError::invalid("kind")),
    };
    let chosen = run_picker(app.clone(), media).await?;
    let mut picked = Vec::with_capacity(chosen.len());
    for entry in chosen {
        let path = platform::picked_path(&app, entry)?;
        let file = register_upload(&state, &path)?;
        picked.push(file);
    }
    Ok(picked)
}

/// Runs the blocking dialog on a worker thread. While it is open the window's focus loss does not
/// count toward the app lock.
async fn run_picker(app: AppHandle, media: bool) -> CmdResult<Vec<FilePath>> {
    let handle = app.clone();
    let chosen = tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = handle.dialog().file();
        if media {
            dialog = dialog.add_filter("Media", MEDIA_EXTENSIONS);
        }
        dialog.blocking_pick_files()
    });
    let state = app.state::<AppState>();
    state.lock_machine().set_external_ui(true);
    let result = chosen.await;
    state.lock_machine().set_external_ui(false);
    Ok(result
        .map_err(|e| CmdError::new(code::CORE, e.to_string()))?
        .unwrap_or_default())
}

/// Canonicalises a picked file, allows it for upload and describes it for the webview.
fn register_upload(state: &AppState, path: &Path) -> CmdResult<PickedFile> {
    let canonical = std::fs::canonicalize(path)?;
    let metadata = std::fs::metadata(&canonical)?;
    if !metadata.is_file() {
        return Err(CmdError::invalid("file"));
    }
    let name = canonical
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_owned();
    let text = canonical
        .to_str()
        .ok_or_else(|| CmdError::invalid("file path"))?
        .to_owned();
    let mime = guess_mime(&name).to_owned();
    state.allow_upload(canonical);
    Ok(PickedFile {
        path: text,
        name,
        mime,
        size: metadata.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_guess_uses_extension_case_insensitively() {
        assert_eq!(guess_mime("Photo.JPG"), "image/jpeg");
        assert_eq!(guess_mime("clip.mov"), "video/quicktime");
        assert_eq!(guess_mime("README"), "application/octet-stream");
        assert_eq!(guess_mime("archive.tar.gz"), "application/octet-stream");
    }

    #[test]
    fn only_http_and_https_urls_open() {
        assert!(check_http_url("https://example.invalid/a").is_ok());
        assert!(check_http_url("http://example.invalid/a").is_ok());
        assert!(check_http_url("file:///etc/passwd").is_err());
        assert!(check_http_url("javascript:alert(1)").is_err());
        assert!(check_http_url("not a url").is_err());
    }

    #[test]
    fn hidden_content_replaces_the_notification_text() {
        let hidden = Prefs {
            hide_notification_content: true,
            ..Prefs::default()
        };
        assert_eq!(
            notification_text(&hidden, "Bob", "secret"),
            ("Chirper".to_owned(), "New message".to_owned())
        );
        assert_eq!(
            notification_text(&Prefs::default(), "Bob", "hi"),
            ("Bob".to_owned(), "hi".to_owned())
        );
    }
}
