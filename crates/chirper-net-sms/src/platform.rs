//! What the platform (on Android, the chirper-native plugin) provides to the SMS driver.
//!
//! The calls block: on Android they go through the Tauri mobile bridge. The driver runs them on
//! `tokio::task::spawn_blocking`, so an implementation may block freely. The types double as the
//! wire format of the plugin, so they are camelCase on the wire.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use chirper_bridge::Result;

/// The platform side of SMS. Implemented by the app on Android; tests implement it with a fake.
pub trait SmsPlatform: Send + Sync + 'static {
    /// The current grant state. Never shows a prompt.
    fn permissions(&self) -> Result<SmsPermissions>;
    /// Shows the system permission prompt and returns the answer.
    fn request_permissions(&self) -> Result<SmsPermissions>;
    /// The most recent conversations, newest first.
    fn threads(&self, limit: usize) -> Result<Vec<SmsThread>>;
    /// Messages of one conversation strictly before `before_ms` (or the newest), newest first.
    fn messages(
        &self,
        thread_id: i64,
        before_ms: Option<i64>,
        limit: usize,
    ) -> Result<Vec<SmsMessage>>;
    /// Messages strictly after `after_ms`, oldest first, at most `limit` of them.
    fn messages_since(&self, after_ms: i64, limit: usize) -> Result<Vec<SmsMessage>>;
    /// Sends one text to each address. Returns a local id that later `SendStatus` events carry.
    fn send_text(
        &self,
        addresses: &[String],
        text: &str,
        subscription_id: Option<i32>,
    ) -> Result<String>;
    /// The bytes of one MMS part. Refuses parts larger than `max_bytes`.
    fn mms_part(&self, part_id: i64, max_bytes: u64) -> Result<Vec<u8>>;
    /// Starts reporting provider changes and send results to `sink`. The sink may be called from
    /// any thread.
    fn watch(&self, sink: Arc<dyn Fn(SmsPlatformEvent) + Send + Sync>) -> Result<()>;
}

/// What the user allowed. The driver needs `read`, `send` and `receive` to chat.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmsPermissions {
    pub read: bool,
    pub send: bool,
    pub receive: bool,
    pub contacts: bool,
}

impl SmsPermissions {
    /// Whether the driver can read and send: the minimum for a usable login.
    pub fn can_chat(&self) -> bool {
        self.read && self.send && self.receive
    }
}

/// A phone number of a participant. Never printed in `Debug`.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmsAddress {
    /// The address as the provider stores it.
    pub raw: String,
    /// The E.164 form, or empty when it could not be determined.
    pub e164: String,
    /// The contact name, or empty.
    pub name: String,
}

impl fmt::Debug for SmsAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SmsAddress([redacted])")
    }
}

/// A conversation as the provider reports it.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmsThread {
    pub thread_id: i64,
    pub addresses: Vec<SmsAddress>,
    pub snippet_date_ms: i64,
}

impl fmt::Debug for SmsThread {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SmsThread")
            .field("thread_id", &self.thread_id)
            .field("addresses", &self.addresses.len())
            .field("snippet_date_ms", &self.snippet_date_ms)
            .finish()
    }
}

/// One SMS or MMS. The body and the addresses are never printed in `Debug`.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmsMessage {
    /// `sms:<_id>` or `mms:<_id>`.
    pub id: String,
    pub thread_id: i64,
    pub sender: SmsAddress,
    pub from_me: bool,
    pub date_ms: i64,
    pub body: String,
    /// The media parts of an MMS. Empty for an SMS.
    pub parts: Vec<MmsPart>,
    /// `received`, `sent`, `sending` or `failed`.
    pub status: String,
}

impl fmt::Debug for SmsMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SmsMessage")
            .field("id", &self.id)
            .field("thread_id", &self.thread_id)
            .field("from_me", &self.from_me)
            .field("date_ms", &self.date_ms)
            .field("body", &format_args!("[{} bytes]", self.body.len()))
            .field("parts", &self.parts.len())
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

/// One media part of an MMS. The bytes are fetched separately with `mms_part`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MmsPart {
    pub part_id: i64,
    pub mime: String,
    pub name: String,
    /// The size in bytes, or -1 when the provider does not know it.
    pub size: i64,
}

/// What the platform reports while watching.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SmsPlatformEvent {
    /// The provider changed: query again since the watermark.
    Changed,
    /// A send reported progress. `status` is `sent`, `delivered` or `failed`.
    SendStatus {
        send_id: String,
        status: String,
        error: String,
    },
}
