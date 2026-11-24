//! The Signal wire messages this driver reads and writes, as prost structs.
//!
//! Ported from signalmeow's `.proto` files (AGPL-3.0, `pkg/signalmeow/protobuf/signalpb/`):
//! `WebSocketResources.proto`, `Provisioning.proto`, `DeviceName.proto` and the used subset of
//! `SignalService.proto`. Tag numbers are copied from those files. Only the fields the driver
//! uses are declared; unknown fields are skipped by prost, as the reference does. Enums are
//! carried as their `int32` values, with the named constants below.

/// `WebSocketMessage.Type`.
pub const WS_TYPE_REQUEST: i32 = 1;
/// `WebSocketMessage.Type`.
pub const WS_TYPE_RESPONSE: i32 = 2;

/// `WebSocketRequestMessage`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct WebSocketRequestMessage {
    #[prost(string, optional, tag = "1")]
    pub verb: Option<String>,
    #[prost(string, optional, tag = "2")]
    pub path: Option<String>,
    #[prost(bytes = "vec", optional, tag = "3")]
    pub body: Option<Vec<u8>>,
    #[prost(string, repeated, tag = "5")]
    pub headers: Vec<String>,
    #[prost(uint64, optional, tag = "4")]
    pub id: Option<u64>,
}

/// `WebSocketResponseMessage`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct WebSocketResponseMessage {
    #[prost(uint64, optional, tag = "1")]
    pub id: Option<u64>,
    #[prost(uint32, optional, tag = "2")]
    pub status: Option<u32>,
    #[prost(string, optional, tag = "3")]
    pub message: Option<String>,
    #[prost(string, repeated, tag = "5")]
    pub headers: Vec<String>,
    #[prost(bytes = "vec", optional, tag = "4")]
    pub body: Option<Vec<u8>>,
}

/// `WebSocketMessage`: one frame of the signal websocket.
#[derive(Clone, PartialEq, prost::Message)]
pub struct WebSocketMessage {
    #[prost(int32, optional, tag = "1")]
    pub r#type: Option<i32>,
    #[prost(message, optional, tag = "2")]
    pub request: Option<WebSocketRequestMessage>,
    #[prost(message, optional, tag = "3")]
    pub response: Option<WebSocketResponseMessage>,
}

/// `ProvisioningAddress`: the UUID of the provisioning session.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ProvisioningAddress {
    #[prost(string, optional, tag = "1")]
    pub address: Option<String>,
}

/// `ProvisionEnvelope`: the encrypted `ProvisionMessage` and the sender's ephemeral key.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ProvisionEnvelope {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub public_key: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "2")]
    pub body: Option<Vec<u8>>,
}

/// `ProvisionMessage`: the account secrets the primary device hands to a linked device.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ProvisionMessage {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub aci_identity_key_public: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "2")]
    pub aci_identity_key_private: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "11")]
    pub pni_identity_key_public: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "12")]
    pub pni_identity_key_private: Option<Vec<u8>>,
    #[prost(string, optional, tag = "8")]
    pub aci: Option<String>,
    #[prost(string, optional, tag = "10")]
    pub pni: Option<String>,
    #[prost(string, optional, tag = "3")]
    pub number: Option<String>,
    #[prost(string, optional, tag = "4")]
    pub provisioning_code: Option<String>,
    #[prost(bytes = "vec", optional, tag = "6")]
    pub profile_key: Option<Vec<u8>>,
    #[prost(uint32, optional, tag = "9")]
    pub provisioning_version: Option<u32>,
    #[prost(bytes = "vec", optional, tag = "14")]
    pub ephemeral_backup_key: Option<Vec<u8>>,
    #[prost(string, optional, tag = "15")]
    pub account_entropy_pool: Option<String>,
    #[prost(bytes = "vec", optional, tag = "16")]
    pub media_root_backup_key: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "17")]
    pub aci_binary: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "18")]
    pub pni_binary: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "19")]
    pub auth_credential_salt: Option<Vec<u8>>,
}

/// `DeviceName`: an encrypted device name, sent when linking.
#[derive(Clone, PartialEq, prost::Message)]
pub struct DeviceName {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub ephemeral_public: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "2")]
    pub synthetic_iv: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "3")]
    pub ciphertext: Option<Vec<u8>>,
}

/// `Envelope`: one message as the server delivers it.
#[derive(Clone, PartialEq, prost::Message)]
pub struct Envelope {
    #[prost(int32, optional, tag = "1")]
    pub r#type: Option<i32>,
    #[prost(string, optional, tag = "11")]
    pub source_service_id: Option<String>,
    #[prost(uint32, optional, tag = "7")]
    pub source_device_id: Option<u32>,
    #[prost(string, optional, tag = "13")]
    pub destination_service_id: Option<String>,
    #[prost(uint64, optional, tag = "5")]
    pub client_timestamp: Option<u64>,
    #[prost(bytes = "vec", optional, tag = "8")]
    pub content: Option<Vec<u8>>,
    #[prost(uint64, optional, tag = "10")]
    pub server_timestamp: Option<u64>,
    #[prost(bytes = "vec", optional, tag = "19")]
    pub source_service_id_binary: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "20")]
    pub destination_service_id_binary: Option<Vec<u8>>,
}

/// `Envelope.Type` values.
pub mod envelope_type {
    pub const UNKNOWN: i32 = 0;
    pub const DOUBLE_RATCHET: i32 = 1;
    pub const PREKEY_MESSAGE: i32 = 3;
    pub const SERVER_DELIVERY_RECEIPT: i32 = 5;
    pub const UNIDENTIFIED_SENDER: i32 = 6;
    pub const PLAINTEXT_CONTENT: i32 = 8;
}

/// `Content`: the plaintext of an encrypted envelope.
#[derive(Clone, PartialEq, prost::Message)]
pub struct Content {
    #[prost(oneof = "content::Content", tags = "1, 2, 5, 6, 8, 11")]
    pub content: Option<content::Content>,
    #[prost(bytes = "vec", optional, tag = "7")]
    pub sender_key_distribution_message: Option<Vec<u8>>,
    #[prost(message, optional, tag = "10")]
    pub pni_signature_message: Option<PniSignatureMessage>,
}

/// Oneof of [`Content`].
pub mod content {
    /// The payload of a `Content` message.
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Content {
        #[prost(message, tag = "1")]
        DataMessage(super::DataMessage),
        #[prost(message, tag = "2")]
        SyncMessage(super::SyncMessage),
        #[prost(message, tag = "5")]
        ReceiptMessage(super::ReceiptMessage),
        #[prost(message, tag = "6")]
        TypingMessage(super::TypingMessage),
        #[prost(bytes, tag = "8")]
        DecryptionErrorMessage(Vec<u8>),
        #[prost(message, tag = "11")]
        EditMessage(super::EditMessage),
    }
}

/// `PniSignatureMessage`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct PniSignatureMessage {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub pni: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "2")]
    pub signature: Option<Vec<u8>>,
}

/// `DataMessage`: a user message, reaction, deletion or attachment.
#[derive(Clone, PartialEq, prost::Message)]
pub struct DataMessage {
    #[prost(string, optional, tag = "1")]
    pub body: Option<String>,
    #[prost(message, repeated, tag = "2")]
    pub attachments: Vec<AttachmentPointer>,
    #[prost(message, optional, tag = "15")]
    pub group_v2: Option<GroupContextV2>,
    #[prost(uint32, optional, tag = "4")]
    pub flags: Option<u32>,
    #[prost(uint32, optional, tag = "5")]
    pub expire_timer: Option<u32>,
    #[prost(bytes = "vec", optional, tag = "6")]
    pub profile_key: Option<Vec<u8>>,
    #[prost(uint64, optional, tag = "7")]
    pub timestamp: Option<u64>,
    #[prost(message, optional, tag = "8")]
    pub quote: Option<Quote>,
    #[prost(message, optional, tag = "11")]
    pub sticker: Option<Sticker>,
    #[prost(bool, optional, tag = "14")]
    pub is_view_once: Option<bool>,
    #[prost(message, optional, tag = "16")]
    pub reaction: Option<Reaction>,
    #[prost(message, optional, tag = "17")]
    pub delete: Option<Delete>,
    #[prost(message, repeated, tag = "18")]
    pub body_ranges: Vec<BodyRange>,
    #[prost(message, optional, tag = "29")]
    pub admin_delete: Option<AdminDelete>,
}

/// `DataMessage.Flags` bits.
pub mod data_flags {
    pub const EXPIRATION_TIMER_UPDATE: u32 = 2;
    pub const PROFILE_KEY_UPDATE: u32 = 4;
}

/// `DataMessage.Quote`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct Quote {
    #[prost(uint64, optional, tag = "1")]
    pub id: Option<u64>,
    #[prost(string, optional, tag = "5")]
    pub author_aci: Option<String>,
    #[prost(string, optional, tag = "3")]
    pub text: Option<String>,
    #[prost(message, repeated, tag = "4")]
    pub attachments: Vec<QuotedAttachment>,
    #[prost(message, repeated, tag = "6")]
    pub body_ranges: Vec<BodyRange>,
    #[prost(int32, optional, tag = "7")]
    pub r#type: Option<i32>,
    #[prost(bytes = "vec", optional, tag = "8")]
    pub author_aci_binary: Option<Vec<u8>>,
}

/// `DataMessage.Quote.QuotedAttachment`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct QuotedAttachment {
    #[prost(string, optional, tag = "1")]
    pub content_type: Option<String>,
    #[prost(string, optional, tag = "2")]
    pub file_name: Option<String>,
    #[prost(message, optional, tag = "3")]
    pub thumbnail: Option<AttachmentPointer>,
}

/// `DataMessage.Reaction`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct Reaction {
    #[prost(string, optional, tag = "1")]
    pub emoji: Option<String>,
    #[prost(bool, optional, tag = "2")]
    pub remove: Option<bool>,
    #[prost(string, optional, tag = "4")]
    pub target_author_aci: Option<String>,
    #[prost(uint64, optional, tag = "5")]
    pub target_sent_timestamp: Option<u64>,
    #[prost(bytes = "vec", optional, tag = "6")]
    pub target_author_aci_binary: Option<Vec<u8>>,
}

/// `DataMessage.Delete`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct Delete {
    #[prost(uint64, optional, tag = "1")]
    pub target_sent_timestamp: Option<u64>,
}

/// `DataMessage.AdminDelete`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct AdminDelete {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub target_author_aci_binary: Option<Vec<u8>>,
    #[prost(uint64, optional, tag = "2")]
    pub target_sent_timestamp: Option<u64>,
}

/// `DataMessage.Sticker`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct Sticker {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub pack_id: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "2")]
    pub pack_key: Option<Vec<u8>>,
    #[prost(uint32, optional, tag = "3")]
    pub sticker_id: Option<u32>,
    #[prost(message, optional, tag = "4")]
    pub data: Option<AttachmentPointer>,
    #[prost(string, optional, tag = "5")]
    pub emoji: Option<String>,
}

/// `AttachmentPointer`. The CDN key is `cdnKey` (tag 15) or the legacy `cdnId` (tag 1).
#[derive(Clone, PartialEq, prost::Message)]
pub struct AttachmentPointer {
    #[prost(oneof = "attachment_pointer::Identifier", tags = "1, 15")]
    pub identifier: Option<attachment_pointer::Identifier>,
    #[prost(bytes = "vec", optional, tag = "20")]
    pub client_uuid: Option<Vec<u8>>,
    #[prost(string, optional, tag = "2")]
    pub content_type: Option<String>,
    #[prost(bytes = "vec", optional, tag = "3")]
    pub key: Option<Vec<u8>>,
    #[prost(uint32, optional, tag = "4")]
    pub size: Option<u32>,
    #[prost(bytes = "vec", optional, tag = "5")]
    pub thumbnail: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "6")]
    pub digest: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "19")]
    pub incremental_mac: Option<Vec<u8>>,
    #[prost(uint32, optional, tag = "17")]
    pub chunk_size: Option<u32>,
    #[prost(string, optional, tag = "7")]
    pub file_name: Option<String>,
    #[prost(uint32, optional, tag = "8")]
    pub flags: Option<u32>,
    #[prost(uint32, optional, tag = "9")]
    pub width: Option<u32>,
    #[prost(uint32, optional, tag = "10")]
    pub height: Option<u32>,
    #[prost(string, optional, tag = "11")]
    pub caption: Option<String>,
    #[prost(string, optional, tag = "12")]
    pub blur_hash: Option<String>,
    #[prost(uint64, optional, tag = "13")]
    pub upload_timestamp: Option<u64>,
    #[prost(uint32, optional, tag = "14")]
    pub cdn_number: Option<u32>,
}

/// Oneof of [`AttachmentPointer`].
pub mod attachment_pointer {
    /// Where the encrypted attachment lives on the CDN.
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Identifier {
        #[prost(fixed64, tag = "1")]
        CdnId(u64),
        #[prost(string, tag = "15")]
        CdnKey(String),
    }
}

/// `BodyRange`: a formatting span or a mention, in UTF-16 units.
#[derive(Clone, PartialEq, prost::Message)]
pub struct BodyRange {
    #[prost(uint32, optional, tag = "1")]
    pub start: Option<u32>,
    #[prost(uint32, optional, tag = "2")]
    pub length: Option<u32>,
    #[prost(oneof = "body_range::Associated", tags = "3, 4, 5")]
    pub associated: Option<body_range::Associated>,
}

/// Oneof of [`BodyRange`].
pub mod body_range {
    /// What a body range means.
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Associated {
        #[prost(string, tag = "3")]
        MentionAci(String),
        #[prost(int32, tag = "4")]
        Style(i32),
        #[prost(bytes, tag = "5")]
        MentionAciBinary(Vec<u8>),
    }
}

/// `BodyRange.Style` values.
pub mod body_style {
    pub const BOLD: i32 = 1;
    pub const ITALIC: i32 = 2;
    pub const SPOILER: i32 = 3;
    pub const STRIKETHROUGH: i32 = 4;
    pub const MONOSPACE: i32 = 5;
}

/// `GroupContextV2`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct GroupContextV2 {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub master_key: Option<Vec<u8>>,
    #[prost(uint32, optional, tag = "2")]
    pub revision: Option<u32>,
    #[prost(bytes = "vec", optional, tag = "3")]
    pub group_change: Option<Vec<u8>>,
}

/// `SyncMessage`: what our other devices did.
#[derive(Clone, PartialEq, prost::Message)]
pub struct SyncMessage {
    #[prost(oneof = "sync_message::Content", tags = "1, 9, 13")]
    pub content: Option<sync_message::Content>,
    #[prost(message, repeated, tag = "5")]
    pub read: Vec<SyncRead>,
    #[prost(message, repeated, tag = "16")]
    pub viewed: Vec<SyncRead>,
}

/// Oneof of [`SyncMessage`].
pub mod sync_message {
    /// The payload of a sync message.
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Content {
        #[prost(message, tag = "1")]
        Sent(super::SyncSent),
        #[prost(message, tag = "9")]
        Configuration(super::SyncConfiguration),
        #[prost(message, tag = "13")]
        Keys(super::SyncKeys),
    }
}

/// `SyncMessage.Sent`: a message our device sent elsewhere.
#[derive(Clone, PartialEq, prost::Message)]
pub struct SyncSent {
    #[prost(string, optional, tag = "1")]
    pub destination_e164: Option<String>,
    #[prost(string, optional, tag = "7")]
    pub destination_service_id: Option<String>,
    #[prost(uint64, optional, tag = "2")]
    pub timestamp: Option<u64>,
    #[prost(message, optional, tag = "3")]
    pub message: Option<DataMessage>,
    #[prost(uint64, optional, tag = "4")]
    pub expiration_start_timestamp: Option<u64>,
    #[prost(message, repeated, tag = "5")]
    pub unidentified_status: Vec<UnidentifiedDeliveryStatus>,
    #[prost(bool, optional, tag = "6")]
    pub is_recipient_update: Option<bool>,
    #[prost(message, optional, tag = "10")]
    pub edit_message: Option<EditMessage>,
    #[prost(bytes = "vec", optional, tag = "12")]
    pub destination_service_id_binary: Option<Vec<u8>>,
}

/// `SyncMessage.Sent.UnidentifiedDeliveryStatus`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct UnidentifiedDeliveryStatus {
    #[prost(string, optional, tag = "3")]
    pub destination_service_id: Option<String>,
    #[prost(bool, optional, tag = "2")]
    pub unidentified: Option<bool>,
    #[prost(bytes = "vec", optional, tag = "5")]
    pub destination_pni_identity_key: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "6")]
    pub destination_service_id_binary: Option<Vec<u8>>,
}

/// `SyncMessage.Read` and `SyncMessage.Viewed` (same shape).
#[derive(Clone, PartialEq, prost::Message)]
pub struct SyncRead {
    #[prost(string, optional, tag = "3")]
    pub sender_aci: Option<String>,
    #[prost(uint64, optional, tag = "2")]
    pub timestamp: Option<u64>,
    #[prost(bytes = "vec", optional, tag = "4")]
    pub sender_aci_binary: Option<Vec<u8>>,
}

/// `SyncMessage.Configuration`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct SyncConfiguration {
    #[prost(bool, optional, tag = "1")]
    pub read_receipts: Option<bool>,
    #[prost(bool, optional, tag = "2")]
    pub unidentified_delivery_indicators: Option<bool>,
    #[prost(bool, optional, tag = "3")]
    pub typing_indicators: Option<bool>,
}

/// `SyncMessage.Keys`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct SyncKeys {
    #[prost(string, optional, tag = "3")]
    pub account_entropy_pool: Option<String>,
    #[prost(bytes = "vec", optional, tag = "4")]
    pub media_root_backup_key: Option<Vec<u8>>,
}

/// `ReceiptMessage`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ReceiptMessage {
    #[prost(int32, optional, tag = "1")]
    pub r#type: Option<i32>,
    #[prost(uint64, repeated, tag = "2")]
    pub timestamp: Vec<u64>,
}

/// `ReceiptMessage.Type` values.
pub mod receipt_type {
    pub const DELIVERY: i32 = 0;
    pub const READ: i32 = 1;
    pub const VIEWED: i32 = 2;
}

/// `TypingMessage`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct TypingMessage {
    #[prost(uint64, optional, tag = "1")]
    pub timestamp: Option<u64>,
    #[prost(int32, optional, tag = "2")]
    pub action: Option<i32>,
    #[prost(bytes = "vec", optional, tag = "3")]
    pub group_id: Option<Vec<u8>>,
}

/// `TypingMessage.Action` values.
pub mod typing_action {
    pub const STARTED: i32 = 0;
    pub const STOPPED: i32 = 1;
}

/// `EditMessage`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct EditMessage {
    #[prost(uint64, optional, tag = "1")]
    pub target_sent_timestamp: Option<u64>,
    #[prost(message, optional, tag = "2")]
    pub data_message: Option<DataMessage>,
}

/// `DecryptionErrorMessage`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct DecryptionErrorMessage {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub ratchet_key: Option<Vec<u8>>,
    #[prost(uint64, optional, tag = "2")]
    pub timestamp: Option<u64>,
    #[prost(uint32, optional, tag = "3")]
    pub device_id: Option<u32>,
}
