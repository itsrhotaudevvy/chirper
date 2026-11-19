//! Android. Haptics and authentication use the mobile plugins. The chirper-native plugin provides
//! the Keystore-wrapped database key, `FLAG_SECURE`, the connection service, message notifications
//! with an inline reply, copies of picked files and the form factor.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use chirper_bridge::{Error as BridgeError, Result as BridgeResult};
use chirper_net_sms::{
    MmsPart, SmsAddress, SmsMessage, SmsPermissions, SmsPlatform, SmsPlatformEvent, SmsThread,
};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter};
use tauri_plugin_chirper_native::{
    ChirperNativeExt, MessageNotice, ReplyEvent, SmsNativeAddress, SmsNativeEvent,
    SmsNativeMessage, SmsNativePart, SmsNativePermissions, SmsNativeThread,
};
use tauri_plugin_dialog::FilePath;
use zeroize::Zeroizing;

use super::mobile;
use crate::error::{CmdError, CmdResult, code};

/// Biometric prompts work on Android through the plugin.
pub const APP_LOCK_SUPPORTED: bool = true;

pub use mobile::{authenticate, haptic};

/// The event that carries an inline reply (CONTRACT.md section C).
const EVENT_NOTIFICATION_REPLY: &str = "notification-reply";
/// The largest file that can be picked and sent.
const MAX_PICKED_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// The reply channel. The native side keeps only its id, so the channel must live as long as the
/// process does.
static REPLY_CHANNEL: OnceLock<Channel<ReplyEvent>> = OnceLock::new();
/// Where the provider's events go: the live SMS session. A new session replaces it.
type SmsSink = Arc<dyn Fn(SmsPlatformEvent) + Send + Sync>;
static SMS_SINK: Mutex<Option<SmsSink>> = Mutex::new(None);
/// The channel of the SMS provider watch. The native side keeps only its id, so the channel must
/// live as long as the process does.
static SMS_CHANNEL: Mutex<Option<Channel<SmsNativeEvent>>> = Mutex::new(None);
/// The newest message notice that waits for the notification permission.
static PENDING_NOTICE: Mutex<Option<MessageNotice>> = Mutex::new(None);
/// Set while the permission dialog is open, so that a burst of messages asks only once.
static ASKING_PERMISSION: AtomicBool = AtomicBool::new(false);

/// The database key, wrapped by the Android Keystore. The first launch creates it.
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

/// Android shows the badge from the app's active notifications, so there is nothing to set.
pub fn set_badge(_app: &AppHandle, _count: i64) -> CmdResult<()> {
    Ok(())
}

/// `FLAG_SECURE` on the activity window: no screenshots and no previews in recents.
pub fn set_secure_screen(app: &AppHandle, enabled: bool) -> CmdResult<()> {
    app.chirper_native()
        .set_secure_screen(enabled)
        .map_err(native_error)
}

/// Runs the foreground service while accounts exist.
pub fn set_background_service(app: &AppHandle, running: bool) -> CmdResult<()> {
    app.chirper_native()
        .set_connection_service(running)
        .map_err(native_error)
}

/// Routes inline replies to the webview as `notification-reply` events. The webview sends the
/// message. A reply that arrives while the webview is not running is lost (the process was killed).
pub fn connect_replies(app: &AppHandle) -> CmdResult<()> {
    let handle = app.clone();
    let channel: Channel<ReplyEvent> = Channel::new(move |body| {
        let Ok(reply) = body.deserialize::<ReplyEvent>() else {
            log::warn!("a notification reply has an unexpected shape");
            return Ok(());
        };
        // The text is a message body: it is passed on and never logged.
        handle.emit(
            EVENT_NOTIFICATION_REPLY,
            serde_json::json!({ "roomId": reply.room_id, "text": reply.text }),
        )
    });
    app.chirper_native()
        .set_reply_channel(&channel)
        .map_err(native_error)?;
    REPLY_CHANNEL
        .set(channel)
        .map_err(|_| CmdError::new(code::CORE, "the reply channel is already connected"))
}

/// Posts the notification of a message. On Android 13 and later the first notice asks for the
/// permission; it is posted when the user allows notifications.
pub fn show_message(
    app: &AppHandle,
    room_id: &str,
    title: &str,
    body: &str,
    hide_content: bool,
) -> CmdResult<()> {
    let notice = MessageNotice {
        room_id: room_id.to_owned(),
        title: title.to_owned(),
        body: body.to_owned(),
        hide_content,
    };
    let outcome = app
        .chirper_native()
        .show_message(&notice)
        .map_err(native_error)?;
    if outcome.permission_missing {
        *lock_pending() = Some(notice);
        ask_notification_permission(app.clone());
    }
    Ok(())
}

/// Asks for the notification permission on a worker thread, because the answer comes from the
/// user's dialog and the main thread must stay free.
fn ask_notification_permission(app: AppHandle) {
    if ASKING_PERMISSION.swap(true, Ordering::SeqCst) {
        return;
    }
    tauri::async_runtime::spawn_blocking(move || {
        let granted = app.chirper_native().request_notification_permission();
        ASKING_PERMISSION.store(false, Ordering::SeqCst);
        let pending = lock_pending().take();
        match granted {
            Ok(true) => {
                if let Some(notice) = pending
                    && let Err(e) = app.chirper_native().show_message(&notice)
                {
                    log::warn!("cannot post the waiting notification: {e}");
                }
            }
            Ok(false) => {}
            Err(e) => log::warn!("cannot ask for the notification permission: {e}"),
        }
    });
}

fn lock_pending() -> std::sync::MutexGuard<'static, Option<MessageNotice>> {
    PENDING_NOTICE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// The local path of a picked file. A `content://` file is copied into the app cache, because the
/// core reads media by path. The copy is refused above 2 GiB.
pub fn picked_path(app: &AppHandle, entry: FilePath) -> CmdResult<PathBuf> {
    match entry {
        FilePath::Path(path) => Ok(path),
        FilePath::Url(url) => {
            let copied = app
                .chirper_native()
                .copy_content_uri(url.as_str(), MAX_PICKED_BYTES)
                .map_err(native_error)?;
            Ok(PathBuf::from(copied.path))
        }
    }
}

pub async fn open_login_webview(
    app: &AppHandle,
    url: &str,
    extract_js: &str,
    wait_for_pattern: &str,
    user_agent: &str,
    title: &str,
) -> CmdResult<crate::commands::LoginWebviewResult> {
    let url_owned = url.to_owned();
    let extract_js_owned = extract_js.to_owned();
    let wait_for_pattern_owned = wait_for_pattern.to_owned();
    let user_agent_owned = user_agent.to_owned();
    let title_owned = title.to_owned();
    let app_handle = app.clone();

    let answer = tauri::async_runtime::spawn_blocking(move || {
        let arg = tauri_plugin_chirper_native::LoginWebviewArg {
            url: &url_owned,
            extract_js: &extract_js_owned,
            wait_for_pattern: &wait_for_pattern_owned,
            user_agent: &user_agent_owned,
            title: &title_owned,
        };
        app_handle.chirper_native().open_login_webview(arg)
    })
    .await
    .map_err(|e| CmdError::new(code::CORE, e.to_string()))?
    .map_err(native_error)?;

    Ok(crate::commands::LoginWebviewResult {
        token: answer.token,
        cancelled: answer.cancelled.unwrap_or(false),
    })
}

fn native_error(err: tauri_plugin_chirper_native::Error) -> CmdError {
    CmdError::new(code::CORE, err.to_string())
}

/// The SMS platform of the Android app: the provider and `SmsManager`, through the chirper-native
/// plugin. Calls block, so the driver runs them on the blocking pool.
pub struct AndroidSms {
    app: AppHandle,
}

impl AndroidSms {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl SmsPlatform for AndroidSms {
    fn permissions(&self) -> BridgeResult<SmsPermissions> {
        self.app
            .chirper_native()
            .sms_permissions()
            .map(permissions_from)
            .map_err(sms_error)
    }

    fn request_permissions(&self) -> BridgeResult<SmsPermissions> {
        self.app
            .chirper_native()
            .request_sms_permissions()
            .map(permissions_from)
            .map_err(sms_error)
    }

    fn threads(&self, limit: usize) -> BridgeResult<Vec<SmsThread>> {
        let threads = self
            .app
            .chirper_native()
            .sms_threads(limit)
            .map_err(sms_error)?;
        Ok(threads.into_iter().map(thread_from).collect())
    }

    fn messages(
        &self,
        thread_id: i64,
        before_ms: Option<i64>,
        limit: usize,
    ) -> BridgeResult<Vec<SmsMessage>> {
        let messages = self
            .app
            .chirper_native()
            .sms_messages(thread_id, before_ms, limit)
            .map_err(sms_error)?;
        Ok(messages.into_iter().map(message_from).collect())
    }

    fn messages_since(&self, after_ms: i64, limit: usize) -> BridgeResult<Vec<SmsMessage>> {
        let messages = self
            .app
            .chirper_native()
            .sms_messages_since(after_ms, limit)
            .map_err(sms_error)?;
        Ok(messages.into_iter().map(message_from).collect())
    }

    fn send_text(
        &self,
        addresses: &[String],
        text: &str,
        subscription_id: Option<i32>,
    ) -> BridgeResult<String> {
        self.app
            .chirper_native()
            .sms_send(addresses, text, subscription_id)
            .map_err(sms_error)
    }

    fn mms_part(&self, part_id: i64, max_bytes: u64) -> BridgeResult<Vec<u8>> {
        self.app
            .chirper_native()
            .sms_mms_part(part_id, max_bytes)
            .map_err(sms_error)
    }

    fn watch(&self, sink: SmsSink) -> BridgeResult<()> {
        *SMS_SINK.lock().unwrap_or_else(PoisonError::into_inner) = Some(sink);
        let mut channel = SMS_CHANNEL.lock().unwrap_or_else(PoisonError::into_inner);
        if channel.is_none() {
            let created: Channel<SmsNativeEvent> = Channel::new(|body| {
                match body.deserialize::<SmsNativeEvent>() {
                    Ok(event) => {
                        let sink = SMS_SINK
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .clone();
                        if let Some(sink) = sink {
                            sink(platform_event(event));
                        }
                    }
                    Err(_) => log::warn!("an SMS provider event has an unexpected shape"),
                }
                Ok(())
            });
            self.app
                .chirper_native()
                .set_sms_channel(&created)
                .map_err(sms_error)?;
            *channel = Some(created);
        }
        Ok(())
    }
}

/// A plugin refusal as a driver error. A missing permission means the grant was revoked, which
/// ends the login; anything else is a refused call. Messages carry no bodies or addresses.
fn sms_error(err: tauri_plugin_chirper_native::Error) -> BridgeError {
    let text = err.to_string();
    if text.contains("SMS permission missing") {
        BridgeError::Auth("SMS permission was revoked".to_owned())
    } else {
        BridgeError::Protocol(format!("the SMS provider refused the call: {text}"))
    }
}

fn permissions_from(state: SmsNativePermissions) -> SmsPermissions {
    SmsPermissions {
        read: state.read,
        send: state.send,
        receive: state.receive,
        contacts: state.contacts,
    }
}

fn address_from(address: SmsNativeAddress) -> SmsAddress {
    SmsAddress {
        raw: address.raw,
        e164: address.e164,
        name: address.name,
    }
}

fn thread_from(thread: SmsNativeThread) -> SmsThread {
    SmsThread {
        thread_id: thread.thread_id,
        addresses: thread.addresses.into_iter().map(address_from).collect(),
        snippet_date_ms: thread.snippet_date_ms,
    }
}

fn part_from(part: SmsNativePart) -> MmsPart {
    MmsPart {
        part_id: part.part_id,
        mime: part.mime,
        name: part.name,
        size: part.size,
    }
}

fn message_from(message: SmsNativeMessage) -> SmsMessage {
    SmsMessage {
        id: message.id,
        thread_id: message.thread_id,
        sender: address_from(message.sender),
        from_me: message.from_me,
        date_ms: message.date_ms,
        body: message.body,
        parts: message.parts.into_iter().map(part_from).collect(),
        status: message.status,
    }
}

fn platform_event(event: SmsNativeEvent) -> SmsPlatformEvent {
    match event {
        SmsNativeEvent::Changed => SmsPlatformEvent::Changed,
        SmsNativeEvent::SendStatus {
            send_id,
            status,
            error,
        } => SmsPlatformEvent::SendStatus {
            send_id,
            status,
            error,
        },
    }
}
