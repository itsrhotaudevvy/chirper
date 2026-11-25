//! The app-state messages: `waServerSync` (patches, snapshots, records) and the `waSyncAction`
//! values the driver reads (contacts, mute, pin, push name).
//!
//! Field numbers are copied from whatsmeow's `proto/waServerSync/WAWebProtobufsServerSync.proto`
//! and `proto/waSyncAction/WAWebProtobufSyncAction.proto` (Mozilla Public License 2.0).

/// `SyncdMutation.SyncdOperation` values.
pub mod operation {
    pub const SET: i32 = 0;
    pub const REMOVE: i32 = 1;
}

/// `waServerSync.SyncdPatch`: one versioned batch of mutations.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SyncdPatch {
    #[prost(message, optional, tag = "1")]
    pub version: Option<SyncdVersion>,
    #[prost(message, repeated, tag = "2")]
    pub mutations: Vec<SyncdMutation>,
    #[prost(message, optional, tag = "3")]
    pub external_mutations: Option<ExternalBlobReference>,
    #[prost(bytes = "vec", optional, tag = "4")]
    pub snapshot_mac: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "5")]
    pub patch_mac: Option<Vec<u8>>,
    #[prost(message, optional, tag = "6")]
    pub key_id: Option<KeyId>,
}

/// `waServerSync.SyncdSnapshot`: the full state of one collection at a version.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SyncdSnapshot {
    #[prost(message, optional, tag = "1")]
    pub version: Option<SyncdVersion>,
    #[prost(message, repeated, tag = "2")]
    pub records: Vec<SyncdRecord>,
    #[prost(bytes = "vec", optional, tag = "3")]
    pub mac: Option<Vec<u8>>,
    #[prost(message, optional, tag = "4")]
    pub key_id: Option<KeyId>,
}

/// `waServerSync.SyncdMutation`: one SET or REMOVE of a record.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SyncdMutation {
    #[prost(int32, optional, tag = "1")]
    pub operation: Option<i32>,
    #[prost(message, optional, tag = "2")]
    pub record: Option<SyncdRecord>,
}

/// `waServerSync.SyncdRecord`.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SyncdRecord {
    #[prost(message, optional, tag = "1")]
    pub index: Option<SyncdBlob>,
    #[prost(message, optional, tag = "2")]
    pub value: Option<SyncdBlob>,
    #[prost(message, optional, tag = "3")]
    pub key_id: Option<KeyId>,
}

/// `SyncdIndex` and `SyncdValue` both hold one `blob` field.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SyncdBlob {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub blob: Option<Vec<u8>>,
}

/// `waServerSync.SyncdVersion`.
#[derive(Clone, Copy, PartialEq, ::prost::Message)]
pub struct SyncdVersion {
    #[prost(uint64, optional, tag = "1")]
    pub version: Option<u64>,
}

/// `waServerSync.KeyId`.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct KeyId {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub id: Option<Vec<u8>>,
}

/// `waServerSync.ExternalBlobReference`: a patch whose mutations live in a downloadable blob.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ExternalBlobReference {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub media_key: Option<Vec<u8>>,
    #[prost(string, optional, tag = "2")]
    pub direct_path: Option<String>,
    #[prost(uint64, optional, tag = "4")]
    pub file_size_bytes: Option<u64>,
    #[prost(bytes = "vec", optional, tag = "6")]
    pub file_enc_sha256: Option<Vec<u8>>,
}

/// `waSyncAction.SyncActionData`: the decrypted content of one mutation.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SyncActionData {
    /// JSON array of strings, for example `["mute","<jid>"]`.
    #[prost(bytes = "vec", optional, tag = "1")]
    pub index: Option<Vec<u8>>,
    #[prost(message, optional, tag = "2")]
    pub value: Option<SyncActionValue>,
    #[prost(int32, optional, tag = "4")]
    pub version: Option<i32>,
}

/// `waSyncAction.SyncActionValue`: the actions the driver reads.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SyncActionValue {
    #[prost(int64, optional, tag = "1")]
    pub timestamp: Option<i64>,
    #[prost(message, optional, tag = "3")]
    pub contact_action: Option<ContactAction>,
    #[prost(message, optional, tag = "4")]
    pub mute_action: Option<MuteAction>,
    #[prost(message, optional, tag = "5")]
    pub pin_action: Option<PinAction>,
    #[prost(message, optional, tag = "7")]
    pub push_name_setting: Option<PushNameSetting>,
    #[prost(message, optional, tag = "17")]
    pub archive_chat_action: Option<ArchiveChatAction>,
}

/// `waSyncAction.ContactAction`.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ContactAction {
    #[prost(string, optional, tag = "1")]
    pub full_name: Option<String>,
    #[prost(string, optional, tag = "2")]
    pub first_name: Option<String>,
    #[prost(string, optional, tag = "3")]
    pub lid_jid: Option<String>,
    #[prost(bool, optional, tag = "4")]
    pub save_on_primary_addressbook: Option<bool>,
    #[prost(string, optional, tag = "5")]
    pub pn_jid: Option<String>,
    #[prost(string, optional, tag = "6")]
    pub username: Option<String>,
}

/// `waSyncAction.MuteAction`.
#[derive(Clone, Copy, PartialEq, ::prost::Message)]
pub struct MuteAction {
    #[prost(bool, optional, tag = "1")]
    pub muted: Option<bool>,
    #[prost(int64, optional, tag = "2")]
    pub mute_end_timestamp: Option<i64>,
}

/// `waSyncAction.PinAction`.
#[derive(Clone, Copy, PartialEq, ::prost::Message)]
pub struct PinAction {
    #[prost(bool, optional, tag = "1")]
    pub pinned: Option<bool>,
}

/// `waSyncAction.ArchiveChatAction`.
#[derive(Clone, Copy, PartialEq, ::prost::Message)]
pub struct ArchiveChatAction {
    #[prost(bool, optional, tag = "1")]
    pub archived: Option<bool>,
}

/// `waSyncAction.PushNameSetting`: the name the account owner chose.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct PushNameSetting {
    #[prost(string, optional, tag = "1")]
    pub name: Option<String>,
}
