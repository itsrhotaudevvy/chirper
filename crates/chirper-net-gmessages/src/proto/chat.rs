//! Protobuf messages of conversations, requests and responses, and the update events, from
//! `libgm/gmproto` (`conversations.proto`, `client.proto`, `events.proto`). Field numbers follow the
//! Go schema; enums are `int32` fields (see the constants next to the users).

use super::pairing::{AuthMessage, Device};

/// `conversations.Contact`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct Contact {
    #[prost(string, tag = "1")]
    pub participant_id: String,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(message, optional, tag = "3")]
    pub number: Option<ContactNumber>,
    #[prost(string, tag = "11")]
    pub contact_id: String,
}

/// `conversations.ContactNumber`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ContactNumber {
    #[prost(int32, tag = "1")]
    pub mysterious_int: i32,
    #[prost(string, tag = "2")]
    pub number: String,
    #[prost(string, tag = "3")]
    pub number2: String,
    #[prost(string, optional, tag = "4")]
    pub formatted_number: Option<String>,
}

/// `conversations.SomeKindOfGroupID`: where a conversation keeps its id when it is not on the top level.
#[derive(Clone, PartialEq, prost::Message)]
pub struct SomeKindOfGroupId {
    #[prost(string, tag = "1")]
    pub conversation_id: String,
    #[prost(message, optional, tag = "2")]
    pub nested_id: Option<some_kind_of_group_id::NestedId>,
    #[prost(string, tag = "3")]
    pub maybe_phone_number: String,
}

/// Nested types of [`SomeKindOfGroupId`].
pub mod some_kind_of_group_id {
    /// `SomeKindOfGroupID.NestedID`.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct NestedId {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(string, tag = "2")]
        pub conference_uri: String,
    }
}

/// `conversations.AnotherMessageID`: the alternative message id some events carry.
#[derive(Clone, PartialEq, prost::Message)]
pub struct AnotherMessageId {
    #[prost(string, tag = "1")]
    pub message_id: String,
    #[prost(string, tag = "2")]
    pub another_id: String,
}

/// `conversations.AnotherLatestMessageReference`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct AnotherLatestMessageReference {
    #[prost(message, optional, tag = "1")]
    pub id: Option<AnotherMessageId>,
    #[prost(int64, tag = "2")]
    pub timestamp_ms: i64,
}

/// `conversations.Message`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct Message {
    #[prost(string, tag = "1")]
    pub message_id: String,
    #[prost(message, optional, tag = "3")]
    pub msg_type: Option<MsgType>,
    #[prost(message, optional, tag = "4")]
    pub message_status: Option<MessageStatus>,
    #[prost(int64, tag = "5")]
    pub timestamp: i64,
    #[prost(string, tag = "7")]
    pub conversation_id: String,
    #[prost(string, tag = "9")]
    pub participant_id: String,
    #[prost(message, repeated, tag = "10")]
    pub message_info: Vec<MessageInfo>,
    #[prost(int64, tag = "11")]
    pub r#type: i64,
    #[prost(string, tag = "12")]
    pub tmp_id: String,
    #[prost(string, optional, tag = "14")]
    pub subject: Option<String>,
    #[prost(message, repeated, tag = "19")]
    pub reactions: Vec<ReactionEntry>,
    #[prost(message, optional, tag = "21")]
    pub reply_message: Option<ReplyMessage>,
    #[prost(message, optional, tag = "23")]
    pub some_kind_of_group_id: Option<SomeKindOfGroupId>,
    #[prost(message, optional, tag = "24")]
    pub another_message_id: Option<AnotherMessageId>,
    #[prost(message, optional, tag = "25")]
    pub sender_participant: Option<Participant>,
}

/// `conversations.ReactionEntry`: one emoji and the participants who used it.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ReactionEntry {
    #[prost(message, optional, tag = "1")]
    pub data: Option<ReactionData>,
    #[prost(string, repeated, tag = "2")]
    pub participant_ids: Vec<String>,
}

/// `conversations.ReactionData`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ReactionData {
    #[prost(string, tag = "1")]
    pub unicode: String,
    /// `EmojiType`: 0 unspecified, 1 like, 2 love, 3 laugh, 4 surprised, 5 sad, 6 angry, 7 dislike,
    /// 8 custom, 9 questioning, 10 crying face, 11 pouting face, 12 red heart, 13 emotify.
    #[prost(int32, tag = "2")]
    pub r#type: i32,
}

/// `conversations.ReplyMessage`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ReplyMessage {
    #[prost(string, tag = "1")]
    pub message_id: String,
    #[prost(string, tag = "2")]
    pub conversation_id: String,
}

/// `conversations.MessageInfo`: one part of a message body.
#[derive(Clone, PartialEq, prost::Message)]
pub struct MessageInfo {
    #[prost(string, optional, tag = "1")]
    pub action_message_id: Option<String>,
    #[prost(oneof = "message_info::Data", tags = "2, 3, 5")]
    pub data: Option<message_info::Data>,
}

/// Oneof of [`MessageInfo`].
pub mod message_info {
    /// The `data` oneof.
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Data {
        #[prost(message, tag = "2")]
        MessageContent(super::MessageContent),
        #[prost(message, tag = "3")]
        MediaContent(super::MediaContent),
        #[prost(message, tag = "5")]
        LocationContent(super::LocationContent),
    }
}

/// `conversations.LocationContent`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct LocationContent {
    #[prost(message, optional, tag = "1")]
    pub media_content: Option<MediaContent>,
    #[prost(message, optional, tag = "2")]
    pub message_content: Option<MessageContent>,
}

/// `conversations.MediaContent`: an attachment and the key that decrypts it.
#[derive(Clone, PartialEq, prost::Message)]
pub struct MediaContent {
    /// `MediaFormats`, e.g. 1 image/jpeg, 8 video/mp4, 15 audio/mpeg.
    #[prost(int32, tag = "1")]
    pub format: i32,
    #[prost(string, tag = "2")]
    pub media_id: String,
    #[prost(string, tag = "4")]
    pub media_name: String,
    #[prost(int64, tag = "5")]
    pub size: i64,
    #[prost(message, optional, tag = "6")]
    pub dimensions: Option<Dimensions>,
    #[prost(bytes = "vec", tag = "7")]
    pub media_data: Vec<u8>,
    #[prost(string, tag = "9")]
    pub thumbnail_media_id: String,
    #[prost(bytes = "vec", tag = "11")]
    pub decryption_key: Vec<u8>,
    #[prost(bytes = "vec", tag = "12")]
    pub thumbnail_decryption_key: Vec<u8>,
    #[prost(string, tag = "14")]
    pub mime_type: String,
}

/// `conversations.Dimensions`.
#[derive(Clone, Copy, PartialEq, prost::Message)]
pub struct Dimensions {
    #[prost(int64, tag = "1")]
    pub width: i64,
    #[prost(int64, tag = "2")]
    pub height: i64,
}

/// `conversations.MessageContent`: a text body.
#[derive(Clone, PartialEq, prost::Message)]
pub struct MessageContent {
    #[prost(string, tag = "1")]
    pub content: String,
}

/// `conversations.MsgType`.
#[derive(Clone, Copy, PartialEq, prost::Message)]
pub struct MsgType {
    #[prost(int64, tag = "1")]
    pub r#type: i64,
}

/// `conversations.MessageStatus`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct MessageStatus {
    /// `MessageStatusType`: 1 outgoing complete, 2 delivered, 11 displayed, 100 incoming complete,
    /// 108 incoming delivered, 109 incoming displayed, 300 deleted.
    #[prost(int32, tag = "2")]
    pub status: i32,
    #[prost(int64, tag = "3")]
    pub sub_code: i64,
    #[prost(string, tag = "4")]
    pub err_msg: String,
    #[prost(string, tag = "5")]
    pub status_text: String,
    #[prost(int64, tag = "6")]
    pub third_code: i64,
}

/// `conversations.Conversation`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct Conversation {
    #[prost(string, tag = "1")]
    pub conversation_id: String,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(message, optional, tag = "4")]
    pub latest_message: Option<LatestMessage>,
    #[prost(int64, tag = "5")]
    pub last_message_timestamp: i64,
    #[prost(bool, tag = "6")]
    pub unread: bool,
    #[prost(bool, tag = "10")]
    pub is_group_chat: bool,
    #[prost(string, tag = "11")]
    pub default_outgoing_id: String,
    /// `ConversationStatus`: 1 active, 2 archived, 3 deleted, 4 keep archived, 5 spam.
    #[prost(int32, tag = "12")]
    pub status: i32,
    #[prost(bool, tag = "13")]
    pub read_only: bool,
    #[prost(string, tag = "15")]
    pub avatar_hex_color: String,
    #[prost(string, tag = "17")]
    pub latest_message_id: String,
    #[prost(int32, tag = "18")]
    pub send_mode: i32,
    #[prost(message, repeated, tag = "20")]
    pub participants: Vec<Participant>,
    #[prost(string, repeated, tag = "21")]
    pub other_participants: Vec<String>,
    /// `ConversationType`: 0 unknown, 1 SMS, 2 RCS.
    #[prost(int32, tag = "22")]
    pub r#type: i32,
    #[prost(bool, tag = "24")]
    pub sub_type: bool,
    #[prost(bool, tag = "26")]
    pub pinned: bool,
    #[prost(int64, tag = "28")]
    pub unknown_timestamp: i64,
    #[prost(bool, tag = "29")]
    pub third_type: bool,
    #[prost(message, optional, tag = "31")]
    pub some_kind_of_group_id: Option<SomeKindOfGroupId>,
    #[prost(message, optional, tag = "33")]
    pub another_latest_message_reference: Option<AnotherLatestMessageReference>,
    #[prost(string, tag = "34")]
    pub group_avatar_url: String,
    #[prost(message, optional, tag = "35")]
    pub latest_message_another_id: Option<AnotherMessageId>,
}

/// `conversations.Participant`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct Participant {
    #[prost(message, optional, tag = "1")]
    pub id: Option<SmallInfo>,
    #[prost(string, tag = "2")]
    pub first_name: String,
    #[prost(string, tag = "3")]
    pub full_name: String,
    #[prost(string, tag = "5")]
    pub avatar_hex_color: String,
    #[prost(bool, tag = "6")]
    pub is_me: bool,
    #[prost(bool, tag = "8")]
    pub is_visible: bool,
    #[prost(string, tag = "10")]
    pub contact_id: String,
    #[prost(int64, tag = "14")]
    pub bs: i64,
    #[prost(string, tag = "15")]
    pub formatted_number: String,
    #[prost(int64, tag = "19")]
    pub some_int1: i64,
    #[prost(int64, tag = "20")]
    pub some_int2: i64,
}

/// `conversations.SmallInfo`: the identifier of a participant.
#[derive(Clone, PartialEq, prost::Message)]
pub struct SmallInfo {
    /// `IdentifierType`: 1 phone, 2 email, 3 bot, 4 unknown sender.
    #[prost(int32, tag = "1")]
    pub r#type: i32,
    #[prost(string, tag = "2")]
    pub number: String,
    #[prost(string, tag = "3")]
    pub participant_id: String,
}

/// `conversations.LatestMessage`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct LatestMessage {
    #[prost(string, tag = "1")]
    pub display_content: String,
    #[prost(int64, tag = "2")]
    pub from_me: i64,
    #[prost(string, tag = "4")]
    pub display_name: String,
    #[prost(message, optional, tag = "5")]
    pub latest_message_status: Option<LatestMessageStatus>,
}

/// `conversations.LatestMessageStatus`.
#[derive(Clone, Copy, PartialEq, prost::Message)]
pub struct LatestMessageStatus {
    #[prost(int64, tag = "1")]
    pub status2: i64,
    #[prost(int32, tag = "2")]
    pub status: i32,
}

/// `client.Cursor`: the position of a page in a list.
#[derive(Clone, PartialEq, prost::Message)]
pub struct Cursor {
    #[prost(string, tag = "1")]
    pub last_item_id: String,
    #[prost(int64, tag = "2")]
    pub last_item_timestamp: i64,
}

/// `client.ListConversationsRequest`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ListConversationsRequest {
    #[prost(int64, tag = "2")]
    pub count: i64,
    /// 1 inbox, 2 archive, 5 spam and blocked.
    #[prost(int32, tag = "4")]
    pub folder: i32,
    #[prost(message, optional, tag = "5")]
    pub cursor: Option<Cursor>,
}

/// `client.ListConversationsResponse`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ListConversationsResponse {
    #[prost(message, repeated, tag = "2")]
    pub conversations: Vec<Conversation>,
    #[prost(bytes = "vec", optional, tag = "3")]
    pub cursor_bytes: Option<Vec<u8>>,
    #[prost(message, optional, tag = "5")]
    pub cursor: Option<Cursor>,
}

/// `client.ListMessagesRequest`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ListMessagesRequest {
    #[prost(string, tag = "2")]
    pub conversation_id: String,
    #[prost(int64, tag = "3")]
    pub count: i64,
    #[prost(message, optional, tag = "5")]
    pub cursor: Option<Cursor>,
}

/// `client.ListMessagesResponse`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ListMessagesResponse {
    #[prost(message, repeated, tag = "2")]
    pub messages: Vec<Message>,
    #[prost(int64, tag = "4")]
    pub total_messages: i64,
    #[prost(message, optional, tag = "5")]
    pub cursor: Option<Cursor>,
}

/// `client.GetConversationRequest`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct GetConversationRequest {
    #[prost(string, tag = "1")]
    pub conversation_id: String,
}

/// `client.GetConversationResponse`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct GetConversationResponse {
    #[prost(message, optional, tag = "1")]
    pub conversation: Option<Conversation>,
}

/// `client.ReplyPayload`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ReplyPayload {
    #[prost(string, tag = "1")]
    pub message_id: String,
}

/// `client.MessagePayloadContent`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct MessagePayloadContent {
    #[prost(message, optional, tag = "1")]
    pub message_content: Option<MessageContent>,
}

/// `client.MessagePayload`: the body of an outgoing message.
#[derive(Clone, PartialEq, prost::Message)]
pub struct MessagePayload {
    #[prost(string, tag = "1")]
    pub tmp_id: String,
    #[prost(message, optional, tag = "6")]
    pub message_payload_content: Option<MessagePayloadContent>,
    #[prost(string, tag = "7")]
    pub conversation_id: String,
    #[prost(string, tag = "9")]
    pub participant_id: String,
    #[prost(message, repeated, tag = "10")]
    pub message_info: Vec<MessageInfo>,
    #[prost(string, tag = "12")]
    pub tmp_id2: String,
}

/// `client.SendMessageRequest`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct SendMessageRequest {
    #[prost(string, tag = "2")]
    pub conversation_id: String,
    #[prost(message, optional, tag = "3")]
    pub message_payload: Option<MessagePayload>,
    #[prost(string, tag = "5")]
    pub tmp_id: String,
    #[prost(bool, tag = "6")]
    pub force_rcs: bool,
    #[prost(message, optional, tag = "8")]
    pub reply: Option<ReplyPayload>,
}

/// `client.SendMessageResponse`: 1 success, 2 to 4 failures.
#[derive(Clone, Copy, PartialEq, prost::Message)]
pub struct SendMessageResponse {
    #[prost(int32, tag = "3")]
    pub status: i32,
}

/// `client.SendReactionRequest`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct SendReactionRequest {
    #[prost(string, tag = "1")]
    pub message_id: String,
    #[prost(message, optional, tag = "2")]
    pub reaction_data: Option<ReactionData>,
    /// 1 add, 2 remove, 3 switch.
    #[prost(int32, tag = "3")]
    pub action: i32,
}

/// `client.SendReactionResponse`.
#[derive(Clone, Copy, PartialEq, prost::Message)]
pub struct SendReactionResponse {
    #[prost(bool, tag = "1")]
    pub success: bool,
}

/// `client.DeleteMessageRequest`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct DeleteMessageRequest {
    #[prost(string, tag = "2")]
    pub message_id: String,
}

/// `client.DeleteMessageResponse`.
#[derive(Clone, Copy, PartialEq, prost::Message)]
pub struct DeleteMessageResponse {
    #[prost(bool, tag = "2")]
    pub success: bool,
}

/// `client.MessageReadRequest`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct MessageReadRequest {
    #[prost(string, tag = "2")]
    pub conversation_id: String,
    #[prost(string, tag = "3")]
    pub message_id: String,
}

/// `client.TypingUpdateRequest`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct TypingUpdateRequest {
    #[prost(message, optional, tag = "2")]
    pub data: Option<typing_update_request::Data>,
}

/// Nested types of [`TypingUpdateRequest`].
pub mod typing_update_request {
    /// `TypingUpdateRequest.Data`.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Data {
        #[prost(string, tag = "1")]
        pub conversation_id: String,
        #[prost(bool, tag = "3")]
        pub typing: bool,
    }
}

/// `client.GetFullSizeImageRequest`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct GetFullSizeImageRequest {
    #[prost(string, tag = "1")]
    pub message_id: String,
    #[prost(string, tag = "2")]
    pub action_message_id: String,
}

/// `client.AttachmentInfo`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct AttachmentInfo {
    #[prost(string, tag = "1")]
    pub attachment_id: String,
    #[prost(bool, tag = "2")]
    pub encrypted: bool,
}

/// `client.DownloadAttachmentRequest`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct DownloadAttachmentRequest {
    #[prost(message, optional, tag = "1")]
    pub info: Option<AttachmentInfo>,
    #[prost(message, optional, tag = "2")]
    pub auth_data: Option<AuthMessage>,
}

/// `client.StartMediaUploadRequest`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct StartMediaUploadRequest {
    #[prost(int64, tag = "1")]
    pub attachment_type: i64,
    #[prost(message, optional, tag = "2")]
    pub auth_data: Option<AuthMessage>,
    #[prost(message, optional, tag = "3")]
    pub mobile: Option<Device>,
}

/// `client.UploadedMedia`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct UploadedMedia {
    #[prost(string, tag = "1")]
    pub media_id: String,
    #[prost(int64, tag = "2")]
    pub media_number: i64,
}

/// `client.UploadMediaResponse`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct UploadMediaResponse {
    #[prost(message, optional, tag = "1")]
    pub media: Option<UploadedMedia>,
    #[prost(string, tag = "2")]
    pub message: String,
}

/// `client.IsBugleDefaultResponse`.
#[derive(Clone, Copy, PartialEq, prost::Message)]
pub struct IsBugleDefaultResponse {
    #[prost(bool, tag = "1")]
    pub success: bool,
}

/// `events.UpdateEvents`: the payload of a `GET_UPDATES` event.
#[derive(Clone, PartialEq, prost::Message)]
pub struct UpdateEvents {
    #[prost(oneof = "update_events::Event", tags = "2, 3, 4, 6, 7, 15")]
    pub event: Option<update_events::Event>,
}

/// Oneof of [`UpdateEvents`].
pub mod update_events {
    /// The `event` oneof.
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Event {
        #[prost(message, tag = "2")]
        ConversationEvent(super::ConversationEvent),
        #[prost(message, tag = "3")]
        MessageEvent(super::MessageEvent),
        #[prost(message, tag = "4")]
        TypingEvent(super::TypingEvent),
        #[prost(message, tag = "6")]
        UserAlertEvent(super::UserAlertEvent),
        #[prost(message, tag = "7")]
        BrowserPresenceCheckEvent(super::EmptyMessage),
        #[prost(message, tag = "15")]
        AccountChange(super::AccountChangeOrSomethingEvent),
    }
}

/// `util.EmptyArr` and the other empty messages of the schemas.
#[derive(Clone, Copy, PartialEq, prost::Message)]
pub struct EmptyMessage {}

/// `events.AccountChangeOrSomethingEvent`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct AccountChangeOrSomethingEvent {
    #[prost(string, tag = "1")]
    pub account: String,
    #[prost(bool, tag = "2")]
    pub enabled: bool,
}

/// `events.ConversationEvent`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ConversationEvent {
    #[prost(message, repeated, tag = "2")]
    pub data: Vec<Conversation>,
}

/// `events.MessageEvent`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct MessageEvent {
    #[prost(message, repeated, tag = "2")]
    pub data: Vec<Message>,
}

/// `events.UserAlertEvent`.
#[derive(Clone, Copy, PartialEq, prost::Message)]
pub struct UserAlertEvent {
    #[prost(int32, tag = "2")]
    pub alert_type: i32,
}

/// `events.TypingEvent`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct TypingEvent {
    #[prost(message, optional, tag = "2")]
    pub data: Option<TypingData>,
}

/// `events.TypingData`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct TypingData {
    #[prost(string, tag = "1")]
    pub conversation_id: String,
    #[prost(message, optional, tag = "2")]
    pub user: Option<TypingUser>,
    /// `TypingTypes`: 0 stopped, 1 started.
    #[prost(int32, tag = "3")]
    pub r#type: i32,
    #[prost(message, optional, tag = "4")]
    pub some_kind_of_group_id: Option<SomeKindOfGroupId>,
}

/// `events.User`: who is typing.
#[derive(Clone, PartialEq, prost::Message)]
pub struct TypingUser {
    #[prost(int64, tag = "1")]
    pub field1: i64,
    #[prost(string, tag = "2")]
    pub number: String,
}

/// `events.EncryptedData2Container`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct EncryptedData2Container {
    #[prost(message, optional, tag = "2")]
    pub account_change: Option<AccountChangeOrSomethingEvent>,
}
