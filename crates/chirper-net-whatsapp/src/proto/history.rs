//! `waHistorySync.HistorySync` and `waWeb.WebMessageInfo`, the subsets the history import reads.
//!
//! Field numbers are copied from whatsmeow's `proto/waHistorySync/WAWebProtobufsHistorySync.proto`
//! and `proto/waWeb/WAWebProtobufsWeb.proto` (Mozilla Public License 2.0).

use super::e2e::Message;

/// `HistorySync.HistorySyncType` values.
pub mod sync_type {
    pub const INITIAL_BOOTSTRAP: i32 = 0;
    pub const FULL: i32 = 2;
    pub const RECENT: i32 = 3;
    pub const PUSH_NAME: i32 = 4;
    pub const NON_BLOCKING_DATA: i32 = 5;
    pub const ON_DEMAND: i32 = 6;
}

/// `waHistorySync.HistorySync`: one blob of the account's history.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct HistorySync {
    #[prost(int32, optional, tag = "1")]
    pub sync_type: Option<i32>,
    #[prost(message, repeated, tag = "2")]
    pub conversations: Vec<Conversation>,
    #[prost(uint32, optional, tag = "5")]
    pub chunk_order: Option<u32>,
    #[prost(uint32, optional, tag = "6")]
    pub progress: Option<u32>,
    #[prost(message, repeated, tag = "7")]
    pub pushnames: Vec<Pushname>,
    #[prost(string, optional, tag = "16")]
    pub companion_meta_nonce: Option<String>,
    #[prost(bytes = "vec", optional, tag = "19")]
    pub nct_salt: Option<Vec<u8>>,
}

/// `waHistorySync.Conversation`: one chat and its messages in this blob.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Conversation {
    /// The chat's JID.
    #[prost(string, optional, tag = "1")]
    pub id: Option<String>,
    #[prost(message, repeated, tag = "2")]
    pub messages: Vec<HistorySyncMsg>,
    #[prost(uint64, optional, tag = "5")]
    pub last_msg_timestamp: Option<u64>,
    #[prost(uint32, optional, tag = "6")]
    pub unread_count: Option<u32>,
    #[prost(bool, optional, tag = "7")]
    pub read_only: Option<bool>,
    #[prost(bool, optional, tag = "8")]
    pub end_of_history_transfer: Option<bool>,
    #[prost(uint32, optional, tag = "9")]
    pub ephemeral_expiration: Option<u32>,
    #[prost(uint64, optional, tag = "12")]
    pub conversation_timestamp: Option<u64>,
    #[prost(string, optional, tag = "13")]
    pub name: Option<String>,
    #[prost(bool, optional, tag = "16")]
    pub archived: Option<bool>,
    #[prost(uint32, optional, tag = "24")]
    pub pinned: Option<u32>,
    #[prost(uint64, optional, tag = "25")]
    pub mute_end_time: Option<u64>,
    #[prost(string, optional, tag = "38")]
    pub display_name: Option<String>,
    #[prost(string, optional, tag = "39")]
    pub pn_jid: Option<String>,
    #[prost(string, optional, tag = "42")]
    pub lid_jid: Option<String>,
}

/// `waHistorySync.HistorySyncMsg`: one message with its order in the conversation.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct HistorySyncMsg {
    #[prost(message, optional, tag = "1")]
    pub message: Option<WebMessageInfo>,
    #[prost(uint64, optional, tag = "2")]
    pub msg_order_id: Option<u64>,
}

/// `waHistorySync.Pushname`: the display name a contact chose.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Pushname {
    #[prost(string, optional, tag = "1")]
    pub id: Option<String>,
    #[prost(string, optional, tag = "2")]
    pub pushname: Option<String>,
}

/// `waWeb.WebMessageInfo`: a stored message with its key, content and metadata.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct WebMessageInfo {
    #[prost(message, optional, tag = "1")]
    pub key: Option<super::MessageKey>,
    #[prost(message, optional, tag = "2")]
    pub message: Option<Message>,
    #[prost(uint64, optional, tag = "3")]
    pub message_timestamp: Option<u64>,
    #[prost(int32, optional, tag = "4")]
    pub status: Option<i32>,
    #[prost(string, optional, tag = "5")]
    pub participant: Option<String>,
    #[prost(string, optional, tag = "19")]
    pub push_name: Option<String>,
    #[prost(int32, optional, tag = "24")]
    pub message_stub_type: Option<i32>,
    #[prost(bytes = "vec", optional, tag = "49")]
    pub message_secret: Option<Vec<u8>>,
}
