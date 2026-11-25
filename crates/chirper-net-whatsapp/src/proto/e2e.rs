//! The `waE2E.Message` subset the driver reads and writes, hand-written as `prost` derives.
//!
//! Field numbers and wire types are copied from `proto/waE2E/WAWebProtobufsE2E.proto` of
//! whatsmeow (Mozilla Public License 2.0, https://github.com/tulir/whatsmeow). Enums are carried as
//! `int32` with their constants next to the message. Fields not listed here are skipped on decode.

/// `waE2E.Message`: the content of one chat message. Only the variants the driver handles are
/// modelled; anything else decodes to an empty message and is reported as unsupported.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Message {
    #[prost(string, optional, tag = "1")]
    pub conversation: Option<String>,
    #[prost(message, optional, tag = "2")]
    pub sender_key_distribution_message: Option<SenderKeyDistributionMessage>,
    #[prost(message, optional, tag = "3")]
    pub image_message: Option<ImageMessage>,
    #[prost(message, optional, tag = "4")]
    pub contact_message: Option<ContactMessage>,
    #[prost(message, optional, tag = "5")]
    pub location_message: Option<LocationMessage>,
    #[prost(message, optional, tag = "6")]
    pub extended_text_message: Option<ExtendedTextMessage>,
    #[prost(message, optional, tag = "7")]
    pub document_message: Option<DocumentMessage>,
    #[prost(message, optional, tag = "8")]
    pub audio_message: Option<AudioMessage>,
    #[prost(message, optional, tag = "9")]
    pub video_message: Option<VideoMessage>,
    #[prost(message, optional, tag = "12")]
    pub protocol_message: Option<ProtocolMessage>,
    #[prost(message, optional, tag = "26")]
    pub sticker_message: Option<StickerMessage>,
    #[prost(message, optional, tag = "31")]
    pub device_sent_message: Option<Box<DeviceSentMessage>>,
    #[prost(message, optional, tag = "35")]
    pub message_context_info: Option<MessageContextInfo>,
    #[prost(message, optional, tag = "37")]
    pub view_once_message: Option<FutureProofMessage>,
    #[prost(message, optional, tag = "40")]
    pub ephemeral_message: Option<FutureProofMessage>,
    #[prost(message, optional, tag = "46")]
    pub reaction_message: Option<ReactionMessage>,
    #[prost(message, optional, tag = "49")]
    pub poll_creation_message: Option<PollCreationMessage>,
    #[prost(message, optional, tag = "53")]
    pub document_with_caption_message: Option<FutureProofMessage>,
    #[prost(message, optional, tag = "55")]
    pub view_once_message_v2: Option<FutureProofMessage>,
    #[prost(message, optional, tag = "58")]
    pub edited_message: Option<FutureProofMessage>,
    #[prost(message, optional, tag = "59")]
    pub view_once_message_v2_extension: Option<FutureProofMessage>,
    #[prost(message, optional, tag = "60")]
    pub poll_creation_message_v2: Option<PollCreationMessage>,
    #[prost(message, optional, tag = "64")]
    pub poll_creation_message_v3: Option<PollCreationMessage>,
    #[prost(message, optional, tag = "66")]
    pub ptv_message: Option<VideoMessage>,
    #[prost(message, optional, tag = "74")]
    pub lottie_sticker_message: Option<FutureProofMessage>,
    #[prost(message, optional, tag = "111")]
    pub poll_creation_message_v5: Option<PollCreationMessage>,
    #[prost(message, optional, tag = "119")]
    pub poll_creation_message_v6: Option<PollCreationMessage>,
}

/// `waE2E.FutureProofMessage`: a wrapper around another message.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct FutureProofMessage {
    #[prost(message, optional, boxed, tag = "1")]
    pub message: Option<Box<Message>>,
}

/// `waE2E.ContextInfo`: reply, forward and mention context shared by most content messages.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ContextInfo {
    #[prost(string, optional, tag = "1")]
    pub stanza_id: Option<String>,
    #[prost(string, optional, tag = "2")]
    pub participant: Option<String>,
    #[prost(message, optional, boxed, tag = "3")]
    pub quoted_message: Option<Box<Message>>,
    #[prost(string, optional, tag = "4")]
    pub remote_jid: Option<String>,
    #[prost(string, repeated, tag = "15")]
    pub mentioned_jid: Vec<String>,
    #[prost(uint32, optional, tag = "21")]
    pub forwarding_score: Option<u32>,
    #[prost(bool, optional, tag = "22")]
    pub is_forwarded: Option<bool>,
    #[prost(uint32, optional, tag = "25")]
    pub expiration: Option<u32>,
    #[prost(string, optional, tag = "34")]
    pub group_subject: Option<String>,
}

/// `waE2E.MessageContextInfo`: per-message secrets and metadata.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct MessageContextInfo {
    #[prost(bytes = "vec", optional, tag = "3")]
    pub message_secret: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "6")]
    pub bot_message_secret: Option<Vec<u8>>,
}

/// `waE2E.ImageMessage`.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ImageMessage {
    #[prost(string, optional, tag = "1")]
    pub url: Option<String>,
    #[prost(string, optional, tag = "2")]
    pub mimetype: Option<String>,
    #[prost(string, optional, tag = "3")]
    pub caption: Option<String>,
    #[prost(bytes = "vec", optional, tag = "4")]
    pub file_sha256: Option<Vec<u8>>,
    #[prost(uint64, optional, tag = "5")]
    pub file_length: Option<u64>,
    #[prost(uint32, optional, tag = "6")]
    pub height: Option<u32>,
    #[prost(uint32, optional, tag = "7")]
    pub width: Option<u32>,
    #[prost(bytes = "vec", optional, tag = "8")]
    pub media_key: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "9")]
    pub file_enc_sha256: Option<Vec<u8>>,
    #[prost(string, optional, tag = "11")]
    pub direct_path: Option<String>,
    #[prost(int64, optional, tag = "12")]
    pub media_key_timestamp: Option<i64>,
    #[prost(bytes = "vec", optional, tag = "16")]
    pub jpeg_thumbnail: Option<Vec<u8>>,
    #[prost(message, optional, tag = "17")]
    pub context_info: Option<ContextInfo>,
}

/// `waE2E.VideoMessage`.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct VideoMessage {
    #[prost(string, optional, tag = "1")]
    pub url: Option<String>,
    #[prost(string, optional, tag = "2")]
    pub mimetype: Option<String>,
    #[prost(bytes = "vec", optional, tag = "3")]
    pub file_sha256: Option<Vec<u8>>,
    #[prost(uint64, optional, tag = "4")]
    pub file_length: Option<u64>,
    #[prost(uint32, optional, tag = "5")]
    pub seconds: Option<u32>,
    #[prost(bytes = "vec", optional, tag = "6")]
    pub media_key: Option<Vec<u8>>,
    #[prost(string, optional, tag = "7")]
    pub caption: Option<String>,
    #[prost(bool, optional, tag = "8")]
    pub gif_playback: Option<bool>,
    #[prost(uint32, optional, tag = "9")]
    pub height: Option<u32>,
    #[prost(uint32, optional, tag = "10")]
    pub width: Option<u32>,
    #[prost(bytes = "vec", optional, tag = "11")]
    pub file_enc_sha256: Option<Vec<u8>>,
    #[prost(string, optional, tag = "13")]
    pub direct_path: Option<String>,
    #[prost(int64, optional, tag = "14")]
    pub media_key_timestamp: Option<i64>,
    #[prost(bytes = "vec", optional, tag = "16")]
    pub jpeg_thumbnail: Option<Vec<u8>>,
    #[prost(message, optional, tag = "17")]
    pub context_info: Option<ContextInfo>,
}

/// `waE2E.AudioMessage`. A push-to-talk voice note has `ptt` set.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct AudioMessage {
    #[prost(string, optional, tag = "1")]
    pub url: Option<String>,
    #[prost(string, optional, tag = "2")]
    pub mimetype: Option<String>,
    #[prost(bytes = "vec", optional, tag = "3")]
    pub file_sha256: Option<Vec<u8>>,
    #[prost(uint64, optional, tag = "4")]
    pub file_length: Option<u64>,
    #[prost(uint32, optional, tag = "5")]
    pub seconds: Option<u32>,
    #[prost(bool, optional, tag = "6")]
    pub ptt: Option<bool>,
    #[prost(bytes = "vec", optional, tag = "7")]
    pub media_key: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "8")]
    pub file_enc_sha256: Option<Vec<u8>>,
    #[prost(string, optional, tag = "9")]
    pub direct_path: Option<String>,
    #[prost(int64, optional, tag = "10")]
    pub media_key_timestamp: Option<i64>,
    #[prost(message, optional, tag = "17")]
    pub context_info: Option<ContextInfo>,
}

/// `waE2E.DocumentMessage`.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct DocumentMessage {
    #[prost(string, optional, tag = "1")]
    pub url: Option<String>,
    #[prost(string, optional, tag = "2")]
    pub mimetype: Option<String>,
    #[prost(string, optional, tag = "3")]
    pub title: Option<String>,
    #[prost(bytes = "vec", optional, tag = "4")]
    pub file_sha256: Option<Vec<u8>>,
    #[prost(uint64, optional, tag = "5")]
    pub file_length: Option<u64>,
    #[prost(bytes = "vec", optional, tag = "7")]
    pub media_key: Option<Vec<u8>>,
    #[prost(string, optional, tag = "8")]
    pub file_name: Option<String>,
    #[prost(bytes = "vec", optional, tag = "9")]
    pub file_enc_sha256: Option<Vec<u8>>,
    #[prost(string, optional, tag = "10")]
    pub direct_path: Option<String>,
    #[prost(int64, optional, tag = "11")]
    pub media_key_timestamp: Option<i64>,
    #[prost(bool, optional, tag = "12")]
    pub contact_vcard: Option<bool>,
    #[prost(bytes = "vec", optional, tag = "16")]
    pub jpeg_thumbnail: Option<Vec<u8>>,
    #[prost(message, optional, tag = "17")]
    pub context_info: Option<ContextInfo>,
    #[prost(string, optional, tag = "20")]
    pub caption: Option<String>,
}

/// `waE2E.StickerMessage`.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct StickerMessage {
    #[prost(string, optional, tag = "1")]
    pub url: Option<String>,
    #[prost(bytes = "vec", optional, tag = "2")]
    pub file_sha256: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "3")]
    pub file_enc_sha256: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "4")]
    pub media_key: Option<Vec<u8>>,
    #[prost(string, optional, tag = "5")]
    pub mimetype: Option<String>,
    #[prost(uint32, optional, tag = "6")]
    pub height: Option<u32>,
    #[prost(uint32, optional, tag = "7")]
    pub width: Option<u32>,
    #[prost(string, optional, tag = "8")]
    pub direct_path: Option<String>,
    #[prost(uint64, optional, tag = "9")]
    pub file_length: Option<u64>,
    #[prost(int64, optional, tag = "10")]
    pub media_key_timestamp: Option<i64>,
    #[prost(bool, optional, tag = "13")]
    pub is_animated: Option<bool>,
    #[prost(bytes = "vec", optional, tag = "16")]
    pub png_thumbnail: Option<Vec<u8>>,
    #[prost(message, optional, tag = "17")]
    pub context_info: Option<ContextInfo>,
}

/// `waE2E.ExtendedTextMessage`: a text message with optional link preview and context.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ExtendedTextMessage {
    #[prost(string, optional, tag = "1")]
    pub text: Option<String>,
    #[prost(string, optional, tag = "2")]
    pub matched_text: Option<String>,
    #[prost(string, optional, tag = "5")]
    pub description: Option<String>,
    #[prost(string, optional, tag = "6")]
    pub title: Option<String>,
    #[prost(bytes = "vec", optional, tag = "16")]
    pub jpeg_thumbnail: Option<Vec<u8>>,
    #[prost(message, optional, tag = "17")]
    pub context_info: Option<ContextInfo>,
}

/// `waE2E.LocationMessage`.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct LocationMessage {
    #[prost(double, optional, tag = "1")]
    pub degrees_latitude: Option<f64>,
    #[prost(double, optional, tag = "2")]
    pub degrees_longitude: Option<f64>,
    #[prost(string, optional, tag = "3")]
    pub name: Option<String>,
    #[prost(string, optional, tag = "4")]
    pub address: Option<String>,
    #[prost(string, optional, tag = "11")]
    pub comment: Option<String>,
    #[prost(message, optional, tag = "17")]
    pub context_info: Option<ContextInfo>,
}

/// `waE2E.ContactMessage`: a shared contact card.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ContactMessage {
    #[prost(string, optional, tag = "1")]
    pub display_name: Option<String>,
    #[prost(string, optional, tag = "16")]
    pub vcard: Option<String>,
    #[prost(message, optional, tag = "17")]
    pub context_info: Option<ContextInfo>,
}

/// `waE2E.PollCreationMessage`: only the question is read.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct PollCreationMessage {
    #[prost(string, optional, tag = "2")]
    pub name: Option<String>,
}

/// `waE2E.ProtocolMessage`: revokes, edits, history and app-state key traffic.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ProtocolMessage {
    #[prost(message, optional, tag = "1")]
    pub key: Option<super::MessageKey>,
    #[prost(int32, optional, tag = "2")]
    pub r#type: Option<i32>,
    #[prost(message, optional, tag = "6")]
    pub history_sync_notification: Option<HistorySyncNotification>,
    #[prost(message, optional, tag = "7")]
    pub app_state_sync_key_share: Option<AppStateSyncKeyShare>,
    #[prost(message, optional, boxed, tag = "14")]
    pub edited_message: Option<Box<Message>>,
    #[prost(int64, optional, tag = "15")]
    pub timestamp_ms: Option<i64>,
    #[prost(message, optional, tag = "16")]
    pub peer_data_operation_request_message: Option<PeerDataOperationRequestMessage>,
}

/// `ProtocolMessage.Type` values.
pub mod protocol_type {
    pub const REVOKE: i32 = 0;
    pub const HISTORY_SYNC_NOTIFICATION: i32 = 5;
    pub const APP_STATE_SYNC_KEY_SHARE: i32 = 6;
    pub const MESSAGE_EDIT: i32 = 14;
    pub const PEER_DATA_OPERATION_REQUEST_MESSAGE: i32 = 16;
}

/// `waE2E.ReactionMessage`. An empty `text` removes the sender's reaction.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ReactionMessage {
    #[prost(message, optional, tag = "1")]
    pub key: Option<super::MessageKey>,
    #[prost(string, optional, tag = "2")]
    pub text: Option<String>,
    #[prost(int64, optional, tag = "4")]
    pub sender_timestamp_ms: Option<i64>,
}

/// `waE2E.SenderKeyDistributionMessage`: a group's sender key, wrapped for the Signal layer.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SenderKeyDistributionMessage {
    #[prost(string, optional, tag = "1")]
    pub group_id: Option<String>,
    #[prost(bytes = "vec", optional, tag = "2")]
    pub axolotl_sender_key_distribution_message: Option<Vec<u8>>,
}

/// `waE2E.DeviceSentMessage`: a message our own phone sent, shown to our other devices.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct DeviceSentMessage {
    #[prost(string, optional, tag = "1")]
    pub destination_jid: Option<String>,
    #[prost(message, optional, boxed, tag = "2")]
    pub message: Option<Box<Message>>,
    #[prost(string, optional, tag = "3")]
    pub phash: Option<String>,
}

/// `waE2E.HistorySyncNotification`: points at a history blob to download.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct HistorySyncNotification {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub file_sha256: Option<Vec<u8>>,
    #[prost(uint64, optional, tag = "2")]
    pub file_length: Option<u64>,
    #[prost(bytes = "vec", optional, tag = "3")]
    pub media_key: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "4")]
    pub file_enc_sha256: Option<Vec<u8>>,
    #[prost(string, optional, tag = "5")]
    pub direct_path: Option<String>,
    #[prost(int32, optional, tag = "6")]
    pub sync_type: Option<i32>,
    #[prost(uint32, optional, tag = "7")]
    pub chunk_order: Option<u32>,
    #[prost(bytes = "vec", optional, tag = "11")]
    pub initial_hist_bootstrap_inline_payload: Option<Vec<u8>>,
    #[prost(string, optional, tag = "14")]
    pub enc_handle: Option<String>,
}

/// `waE2E.AppStateSyncKeyShare`: the phone hands us the keys of the app-state collections.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct AppStateSyncKeyShare {
    #[prost(message, repeated, tag = "1")]
    pub keys: Vec<AppStateSyncKey>,
}

/// `waE2E.AppStateSyncKey`.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct AppStateSyncKey {
    #[prost(message, optional, tag = "1")]
    pub key_id: Option<AppStateSyncKeyId>,
    #[prost(message, optional, tag = "2")]
    pub key_data: Option<AppStateSyncKeyData>,
}

/// `waE2E.AppStateSyncKeyId`.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct AppStateSyncKeyId {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub key_id: Option<Vec<u8>>,
}

/// `waE2E.AppStateSyncKeyData`.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct AppStateSyncKeyData {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub key_data: Option<Vec<u8>>,
    #[prost(int64, optional, tag = "3")]
    pub timestamp: Option<i64>,
}

/// `waE2E.PeerDataOperationRequestMessage`: a request our primary phone answers.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct PeerDataOperationRequestMessage {
    #[prost(int32, optional, tag = "1")]
    pub peer_data_operation_request_type: Option<i32>,
    #[prost(message, optional, tag = "4")]
    pub history_sync_on_demand_request: Option<HistorySyncOnDemandRequest>,
}

/// `PeerDataOperationRequestMessage.HistorySyncOnDemandRequest`.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct HistorySyncOnDemandRequest {
    #[prost(string, optional, tag = "1")]
    pub chat_jid: Option<String>,
    #[prost(string, optional, tag = "2")]
    pub oldest_msg_id: Option<String>,
    #[prost(bool, optional, tag = "3")]
    pub oldest_msg_from_me: Option<bool>,
    #[prost(int32, optional, tag = "4")]
    pub on_demand_msg_count: Option<i32>,
    #[prost(int64, optional, tag = "5")]
    pub oldest_msg_timestamp_ms: Option<i64>,
    #[prost(bool, optional, tag = "7")]
    pub support_inline_response: Option<bool>,
    #[prost(int32, optional, tag = "8")]
    pub mode: Option<i32>,
}

/// `PeerDataOperationRequestType.HISTORY_SYNC_ON_DEMAND`.
pub const PEER_HISTORY_SYNC_ON_DEMAND: i32 = 3;

/// The `waE2E.Message` a `ProtocolMessage` of type `PEER_DATA_OPERATION_REQUEST_MESSAGE` carries.
pub fn history_sync_request(request: HistorySyncOnDemandRequest) -> Message {
    Message {
        protocol_message: Some(ProtocolMessage {
            r#type: Some(protocol_type::PEER_DATA_OPERATION_REQUEST_MESSAGE),
            peer_data_operation_request_message: Some(PeerDataOperationRequestMessage {
                peer_data_operation_request_type: Some(PEER_HISTORY_SYNC_ON_DEMAND),
                history_sync_on_demand_request: Some(request),
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}
