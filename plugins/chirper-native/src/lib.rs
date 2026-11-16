//! Chirper's native mobile integration. The Kotlin code (`android/`) and the Swift code (`ios/`) do
//! the platform work; this crate is the typed Rust side that the app calls with
//! `run_mobile_plugin`. Nothing is exposed to the webview, so the plugin has no permissions.
//!
//! On desktop the crate is empty: the app only registers the plugin on Android and iOS.

#[cfg(any(target_os = "android", test))]
mod keyfile;

#[cfg(any(target_os = "android", target_os = "ios"))]
mod db_key;
#[cfg(any(target_os = "android", target_os = "ios"))]
mod mobile;

#[cfg(any(target_os = "android", target_os = "ios"))]
pub use db_key::{DB_KEY_LEN, load_or_create_db_key};
#[cfg(target_os = "android")]
pub use mobile::CopiedFile;
#[cfg(any(target_os = "android", target_os = "ios"))]
pub use mobile::{
    ChirperNative, ChirperNativeExt, FormFactor, LoginWebviewAnswer, LoginWebviewArg,
    MessageNotice, MessageOutcome, ReplyEvent, init,
};

/// Errors of the native calls. Messages never contain message bodies, keys or user data.
#[cfg(any(target_os = "android", target_os = "ios"))]
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(any(target_os = "android", target_os = "ios"))]
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the native side refused the call: {0}")]
    Native(#[from] tauri::plugin::mobile::PluginInvokeError),
    #[error("cannot read or write a key file: {0}")]
    Io(#[from] std::io::Error),
    #[cfg(target_os = "android")]
    #[error("{0}")]
    Format(#[from] keyfile::FormatError),
    #[error("the random source failed")]
    Random,
    #[error("{0}")]
    Invalid(&'static str),
}
