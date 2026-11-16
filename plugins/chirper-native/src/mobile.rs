//! Typed wrappers over the Kotlin (`app.chirper.platform.ChirperNativePlugin`) and Swift
//! (`ChirperNativePlugin`) commands. Calls block until the native side answers, so callers must not
//! be waiting on the main thread for something the native side has to do on it. Permission prompts
//! are therefore requested from a worker thread by the app.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{
    Manager, Runtime,
    ipc::Channel,
    plugin::{Builder, PluginApi, PluginHandle, TauriPlugin},
};
use zeroize::Zeroizing;

use crate::Result;

pub const PLUGIN_NAME: &str = "chirper-native";

#[cfg(target_os = "android")]
const ANDROID_PACKAGE: &str = "app.chirper.platform";
#[cfg(target_os = "android")]
const ANDROID_CLASS: &str = "ChirperNativePlugin";

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_chirper_native);

/// Registers the plugin. The app adds it on Android and iOS only.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new(PLUGIN_NAME)
        .setup(|app, api| {
            let native = ChirperNative::register(api)?;
            app.manage(native);
            Ok(())
        })
        .build()
}

/// Access to the managed [`ChirperNative`] from an app handle or any other Tauri manager.
pub trait ChirperNativeExt<R: Runtime> {
    fn chirper_native(&self) -> &ChirperNative<R>;
}

impl<R: Runtime, T: Manager<R>> ChirperNativeExt<R> for T {
    fn chirper_native(&self) -> &ChirperNative<R> {
        self.state::<ChirperNative<R>>().inner()
    }
}

/// The handle to the native plugin of this platform.
pub struct ChirperNative<R: Runtime>(PluginHandle<R>);

/// `formFactor` of `app_info`: `phone` or `tablet`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FormFactor {
    Phone,
    Tablet,
}

/// A message to show as a notification. `hide_content` asks for a generic notification without text.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageNotice {
    pub room_id: String,
    pub title: String,
    pub body: String,
    pub hide_content: bool,
}

/// What the native side did with a [`MessageNotice`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageOutcome {
    pub posted: bool,
    /// The user has not allowed notifications yet, so the notice was not posted.
    pub permission_missing: bool,
}

/// An inline reply typed in a notification, delivered by the native side on a channel.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplyEvent {
    pub room_id: String,
    pub text: String,
}

/// A content URI copied into the app cache.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[cfg(target_os = "android")]
pub struct CopiedFile {
    pub path: String,
    pub size: u64,
}

/// Bytes travel as standard base64 inside the JSON the plugin bridge uses.
mod b64 {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        bytes: &[u8],
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        STANDARD.decode(text).map_err(serde::de::Error::custom)
    }
}

/// An AES-GCM wrapped value: the IV and the ciphertext with its tag.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg(target_os = "android")]
pub struct Wrapped {
    #[serde(with = "b64")]
    pub iv: Vec<u8>,
    #[serde(with = "b64")]
    pub ciphertext: Vec<u8>,
}

#[derive(Serialize)]
#[cfg(target_os = "android")]
struct PlaintextArg<'a> {
    #[serde(with = "b64")]
    plaintext: &'a [u8],
}

#[derive(Deserialize)]
#[cfg(target_os = "android")]
struct PlaintextOut {
    #[serde(with = "b64")]
    plaintext: Vec<u8>,
}

#[derive(Deserialize)]
#[cfg(target_os = "android")]
struct PathOut {
    path: String,
}

#[derive(Serialize)]
struct EnabledArg {
    enabled: bool,
}

#[derive(Serialize)]
#[cfg(target_os = "android")]
struct RunningArg {
    running: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg(target_os = "android")]
struct CopyArg<'a> {
    uri: &'a str,
    max_bytes: u64,
}

#[derive(Serialize)]
#[cfg(target_os = "android")]
struct ReplyChannelArg<'a> {
    handler: &'a Channel<ReplyEvent>,
}

#[derive(Deserialize)]
struct FormFactorAnswer {
    #[serde(rename = "formFactor")]
    form_factor: FormFactor,
}

#[derive(Deserialize)]
#[cfg(target_os = "android")]
struct PermissionAnswer {
    granted: bool,
}

#[derive(Serialize)]
#[cfg(target_os = "ios")]
struct KeychainArg<'a> {
    service: &'a str,
    account: &'a str,
    #[serde(with = "b64")]
    value: &'a [u8],
}

#[derive(Deserialize)]
#[cfg(target_os = "ios")]
struct KeychainAnswer {
    #[serde(with = "b64")]
    value: Vec<u8>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginWebviewArg<'a> {
    pub url: &'a str,
    pub extract_js: &'a str,
    pub wait_for_pattern: &'a str,
    pub user_agent: &'a str,
    pub title: &'a str,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LoginWebviewAnswer {
    pub token: Option<String>,
    pub cancelled: Option<bool>,
}

#[derive(Serialize)]
#[cfg(target_os = "ios")]
struct BadgeArg {
    count: i64,
}

impl<R: Runtime> ChirperNative<R> {
    fn register<C: serde::de::DeserializeOwned>(api: PluginApi<R, C>) -> Result<Self> {
        #[cfg(target_os = "android")]
        let handle = api.register_android_plugin(ANDROID_PACKAGE, ANDROID_CLASS)?;
        #[cfg(target_os = "ios")]
        let handle = api.register_ios_plugin(init_chirper_native)?;
        Ok(Self(handle))
    }

    /// The form factor of the device. Tablets are Android devices with a smallest width of at
    /// least 600 dp and iPads.
    pub fn form_factor(&self) -> Result<FormFactor> {
        let answer: FormFactorAnswer = self.0.run_mobile_plugin("formFactor", ())?;
        Ok(answer.form_factor)
    }

    /// Sets or clears the secure-screen protection of the app window (`FLAG_SECURE` on Android,
    /// the blur and capture overlay on iOS).
    pub fn set_secure_screen(&self, enabled: bool) -> Result<()> {
        self.0
            .run_mobile_plugin::<()>("setSecureScreen", EnabledArg { enabled })?;
        Ok(())
    }

    /// Encrypts `plaintext` with the Keystore key `chirper-db-wrap`.
    #[cfg(target_os = "android")]
    pub fn wrap_key(&self, plaintext: &[u8]) -> Result<Wrapped> {
        Ok(self
            .0
            .run_mobile_plugin("wrapKey", PlaintextArg { plaintext })?)
    }

    /// Decrypts a value made by [`ChirperNative::wrap_key`]. The result is wiped when dropped.
    #[cfg(target_os = "android")]
    pub fn unwrap_key(&self, wrapped: &Wrapped) -> Result<Zeroizing<Vec<u8>>> {
        let out: PlaintextOut = self.0.run_mobile_plugin("unwrapKey", wrapped)?;
        Ok(Zeroizing::new(out.plaintext))
    }

    /// The app's no-backup files directory, where the wrapped key is kept.
    #[cfg(target_os = "android")]
    pub fn no_backup_dir(&self) -> Result<PathBuf> {
        let out: PathOut = self.0.run_mobile_plugin("noBackupDir", ())?;
        Ok(PathBuf::from(out.path))
    }

    /// Runs the foreground service that keeps the process alive while accounts are connected.
    #[cfg(target_os = "android")]
    pub fn set_connection_service(&self, running: bool) -> Result<()> {
        self.0
            .run_mobile_plugin::<()>("setConnectionService", RunningArg { running })?;
        Ok(())
    }

    /// Posts or updates the message notification of a room.
    #[cfg(target_os = "android")]
    pub fn show_message(&self, notice: &MessageNotice) -> Result<MessageOutcome> {
        Ok(self.0.run_mobile_plugin("showMessage", notice)?)
    }

    /// Asks the user for permission to post notifications. Blocks until the user answers, so call
    /// it from a worker thread.
    #[cfg(target_os = "android")]
    pub fn request_notification_permission(&self) -> Result<bool> {
        let answer: PermissionAnswer = self
            .0
            .run_mobile_plugin("requestNotificationPermission", ())?;
        Ok(answer.granted)
    }

    /// Registers the channel that receives inline replies. The channel must stay alive.
    #[cfg(target_os = "android")]
    pub fn set_reply_channel(&self, channel: &Channel<ReplyEvent>) -> Result<()> {
        self.0
            .run_mobile_plugin::<()>("setReplyChannel", ReplyChannelArg { handler: channel })?;
        Ok(())
    }

    /// Copies a `content://` URI chosen in the picker into the app cache. Fails when the file is
    /// larger than `max_bytes`.
    #[cfg(target_os = "android")]
    pub fn copy_content_uri(&self, uri: &str, max_bytes: u64) -> Result<CopiedFile> {
        Ok(self
            .0
            .run_mobile_plugin("copyContentUri", CopyArg { uri, max_bytes })?)
    }

    /// Opens an in-app native web view dialog for web/cookie login flows.
    #[cfg(target_os = "android")]
    pub fn open_login_webview(&self, arg: LoginWebviewArg<'_>) -> Result<LoginWebviewAnswer> {
        Ok(self.0.run_mobile_plugin("openLoginWebview", arg)?)
    }

    /// Stores `value` as the Keychain item of `service` and `account` unless one exists, and returns
    /// the stored value. The item is `ThisDeviceOnly` and readable after the first unlock.
    #[cfg(target_os = "ios")]
    pub fn keychain_store_if_absent(
        &self,
        service: &str,
        account: &str,
        value: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>> {
        let answer: KeychainAnswer = self.0.run_mobile_plugin(
            "keychainStoreIfAbsent",
            KeychainArg {
                service,
                account,
                value,
            },
        )?;
        Ok(Zeroizing::new(answer.value))
    }

    /// Sets the app icon badge (iOS 16 and later; older systems use the icon badge number).
    #[cfg(target_os = "ios")]
    pub fn set_badge(&self, count: i64) -> Result<()> {
        self.0
            .run_mobile_plugin::<()>("setBadge", BadgeArg { count })?;
        Ok(())
    }
}
