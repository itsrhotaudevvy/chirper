//! Serde mirrors of the JSON model in docs/CONTRACT.md section A. The authority is
//! `go/core/engine/types.go`, `go/core/engine/settings.go` and `go/core/store/types.go`.
//! Unknown enum values decode to `Unknown` so that a newer core cannot break an older shell.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;

/// Event kinds emitted by the core (CONTRACT.md section A).
pub mod event_kind {
    pub const CHAT: &str = "chat";
    pub const CHAT_REMOVED: &str = "chat_removed";
    pub const MESSAGE: &str = "message";
    pub const MESSAGE_REMOVED: &str = "message_removed";
    pub const TYPING: &str = "typing";
    pub const ACCOUNT: &str = "account";
    pub const CORE_ERROR: &str = "core_error";
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Network {
    pub id: String,
    pub name: String,
    pub color: String,
    pub beta: bool,
    pub available: bool,
    pub reason: String,
    pub login_flows: Vec<LoginFlow>,
    pub capabilities: Capabilities,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LoginFlow {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Capabilities {
    pub edits: bool,
    pub reactions: bool,
    pub replies: bool,
    pub redactions: bool,
    pub typing: bool,
    pub read_receipts: bool,
    pub stickers: bool,
    pub voice: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AccountState {
    Connected,
    Connecting,
    BadCredentials,
    TransientDisconnect,
    UnknownError,
    LoggedOut,
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Account {
    pub network: String,
    pub login_id: String,
    pub name: String,
    pub avatar_url: String,
    pub state: AccountState,
    pub error: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LoginState {
    pub process_id: String,
    pub step: LoginStep,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginStepType {
    UserInput,
    Cookies,
    DisplayAndWait,
    Complete,
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LoginStep {
    #[serde(rename = "type")]
    pub kind: LoginStepType,
    pub step_id: String,
    pub instructions: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_input: Option<UserInputStep>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cookies: Option<CookiesStep>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_and_wait: Option<DisplayStep>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub complete: Option<CompleteStep>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UserInputStep {
    pub fields: Vec<InputField>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InputField {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub description: String,
    pub options: Vec<String>,
    pub pattern: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CookiesStep {
    pub url: String,
    pub user_agent: String,
    pub fields: Vec<CookieField>,
    pub extract_js: String,
    pub wait_for_url_pattern: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CookieField {
    pub id: String,
    pub required: bool,
    pub sources: Vec<CookieSource>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CookieSource {
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    pub cookie_domain: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DisplayKind {
    Qr,
    Emoji,
    Code,
    Nothing,
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DisplayStep {
    #[serde(rename = "type")]
    pub kind: DisplayKind,
    pub data: String,
    pub image_url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CompleteStep {
    pub login_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Chat {
    pub room_id: String,
    pub network: String,
    pub login_id: String,
    pub name: String,
    pub avatar_url: String,
    pub topic: String,
    pub is_dm: bool,
    pub is_group: bool,
    pub is_space: bool,
    pub member_count: i64,
    pub last_activity: i64,
    pub preview: Option<ChatPreview>,
    pub unread_count: i64,
    pub mention_count: i64,
    pub pinned: bool,
    pub archived: bool,
    pub muted_until: i64,
    pub typing: Vec<String>,
    pub parent_id: String,
    pub server_name: String,
    pub is_channel: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatPreview {
    pub sender: String,
    pub body: String,
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageStatus {
    Sending,
    Sent,
    Delivered,
    Read,
    Failed,
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Message {
    pub event_id: String,
    pub room_id: String,
    pub order: i64,
    pub sender: String,
    pub sender_name: String,
    pub sender_avatar: String,
    pub from_me: bool,
    pub ts: i64,
    #[serde(rename = "type")]
    pub kind: String,
    pub body: String,
    pub formatted_body: String,
    pub media: Option<Media>,
    pub reply_to: Option<ReplyTo>,
    pub edited: bool,
    pub redacted: bool,
    pub reactions: Vec<Reaction>,
    pub status: MessageStatus,
    pub error: String,
    pub live: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Media {
    pub url: String,
    pub mime: String,
    pub size: i64,
    pub width: i64,
    pub height: i64,
    pub duration_ms: i64,
    pub file_name: String,
    pub thumbnail_url: String,
    pub blurhash: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReplyTo {
    pub event_id: String,
    pub sender_name: String,
    pub body: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Reaction {
    pub key: String,
    pub count: i64,
    pub from_me: bool,
    pub senders: Vec<String>,
}

/// Filter for `listChats`. An empty `network` lists every network.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatFilter {
    pub network: String,
    pub archived: bool,
    pub unread_only: bool,
    pub query: String,
    pub direct_only: bool,
    pub servers_only: bool,
}

/// Local-only chat flags for `setChatFlags`. Absent (`None`) fields are left unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatFlags {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub muted_until: Option<i64>,
}

/// User settings (`engine.Settings`). Per-network maps use "*" as the default key and are kept as
/// JSON values so that new networks need no shell change.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CoreSettings {
    pub demo_network: bool,
    pub send_read_receipts: Value,
    pub send_typing: Value,
    pub telegram: Value,
}

/// One event pushed by the core. `payload` is the raw JSON document of the event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreEvent {
    pub kind: String,
    pub payload: Box<RawValue>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const NETWORK: &str = r##"{"id":"telegram","name":"Telegram","color":"#2AABEE","beta":false,
        "loginFlows":[{"id":"phone","name":"Phone number","description":"Log in with a code"}],
        "capabilities":{"edits":true,"reactions":true,"replies":true,"redactions":true,
        "typing":true,"readReceipts":true,"stickers":true,"voice":false},
        "available":true,"reason":""}"##;

    const ACCOUNT: &str = r##"{"network":"discord","loginId":"discord_123","name":"Alice",
        "avatarUrl":"mxc://chirper.local/abc_DEF-1","state":"BAD_CREDENTIALS","error":"expired",
        "message":"Log in again"}"##;

    const LOGIN_INPUT: &str = r##"{"processId":"p-1","step":{"type":"user_input","stepId":"s-2",
        "instructions":"Enter your phone","userInput":{"fields":[{"id":"phone","name":"Phone",
        "type":"text","description":"International format","options":[],"pattern":"^\\+[0-9]+$"}]}}}"##;

    const LOGIN_COOKIES: &str = r##"{"processId":"p-1","step":{"type":"cookies","stepId":"s-3",
        "instructions":"Log in in the browser","cookies":{"url":"https://example.invalid/login",
        "userAgent":"Mozilla/5.0","fields":[{"id":"token","required":true,"sources":[{"type":"cookie",
        "name":"session","cookieDomain":".example.invalid"}]}],"extractJs":"return 1"}}}"##;

    const LOGIN_QR: &str = r##"{"processId":"p-1","step":{"type":"display_and_wait","stepId":"s-4",
        "instructions":"Scan","displayAndWait":{"type":"qr","data":"otpauth://x","imageUrl":""}}}"##;

    const LOGIN_DONE: &str = r##"{"processId":"p-1","step":{"type":"complete","stepId":"s-5",
        "instructions":"","complete":{"loginId":"discord_123"}}}"##;

    const CHAT: &str = r##"{"roomId":"!abc123:chirper.local","network":"discord","loginId":"discord_123",
        "name":"General","avatarUrl":"","topic":"","isDm":false,"isGroup":true,"isSpace":false,
        "memberCount":12,"lastActivity":1700000000000,"preview":{"sender":"@u:chirper.local",
        "body":"hi","type":"m.text"},"unreadCount":3,"mentionCount":1,"pinned":true,
        "archived":false,"mutedUntil":0,"typing":["Bob"]}"##;

    const MESSAGE: &str = r##"{"eventId":"$ev1.abc","roomId":"!abc123:chirper.local","order":42,
        "sender":"@u:chirper.local","senderName":"Bob","senderAvatar":"","fromMe":false,
        "ts":1700000000123,"type":"m.image","body":"photo.jpg","formattedBody":"",
        "media":{"url":"mxc://chirper.local/Qx_9-a","mime":"image/jpeg","size":2048,"width":640,
        "height":480,"durationMs":0,"fileName":"photo.jpg","thumbnailUrl":"","blurhash":"LEHV6n"},
        "replyTo":{"eventId":"$ev0","senderName":"Alice","body":"hello"},"edited":true,
        "redacted":false,"reactions":[{"key":"👍","count":2,"fromMe":true,"senders":["Me","Bob"]}],
        "status":"delivered","error":"","live":true}"##;

    const SETTINGS: &str = r##"{"demoNetwork":false,"sendReadReceipts":{"*":true,"discord":false},
        "sendTyping":{"*":true},"telegram":{"apiId":0,"apiHash":""}}"##;

    #[test]
    fn network_round_trip() {
        let n: Network = serde_json::from_str(NETWORK).unwrap();
        assert_eq!(n.id, "telegram");
        assert!(n.capabilities.read_receipts);
        assert!(!n.capabilities.voice);
        assert_eq!(n.login_flows[0].id, "phone");
        let back: Network = serde_json::from_str(&serde_json::to_string(&n).unwrap()).unwrap();
        assert_eq!(back, n);
    }

    #[test]
    fn account_state_maps_screaming_case() {
        let a: Account = serde_json::from_str(ACCOUNT).unwrap();
        assert_eq!(a.state, AccountState::BadCredentials);
        assert_eq!(a.login_id, "discord_123");
        assert!(
            serde_json::to_string(&a)
                .unwrap()
                .contains("\"BAD_CREDENTIALS\"")
        );
    }

    #[test]
    fn unknown_enum_values_fall_back() {
        let a: Account = serde_json::from_str(r##"{"state":"SOMETHING_NEW"}"##).unwrap();
        assert_eq!(a.state, AccountState::Unknown);
        let m: Message = serde_json::from_str(r##"{"status":"pending"}"##).unwrap();
        assert_eq!(m.status, MessageStatus::Unknown);
    }

    #[test]
    fn login_steps_decode_every_variant() {
        let input: LoginState = serde_json::from_str(LOGIN_INPUT).unwrap();
        assert_eq!(input.step.kind, LoginStepType::UserInput);
        let fields = &input.step.user_input.as_ref().unwrap().fields;
        assert_eq!(fields[0].kind, "text");
        assert_eq!(fields[0].pattern, r"^\+[0-9]+$");

        let cookies: LoginState = serde_json::from_str(LOGIN_COOKIES).unwrap();
        let c = cookies.step.cookies.unwrap();
        assert_eq!(c.extract_js, "return 1");
        assert_eq!(c.fields[0].sources[0].cookie_domain, ".example.invalid");

        let qr: LoginState = serde_json::from_str(LOGIN_QR).unwrap();
        let d = qr.step.display_and_wait.unwrap();
        assert_eq!(d.kind, DisplayKind::Qr);

        let done: LoginState = serde_json::from_str(LOGIN_DONE).unwrap();
        assert_eq!(done.step.kind, LoginStepType::Complete);
        assert_eq!(done.step.complete.unwrap().login_id, "discord_123");
    }

    #[test]
    fn chat_and_preview_decode() {
        let c: Chat = serde_json::from_str(CHAT).unwrap();
        assert!(c.is_group);
        assert_eq!(c.unread_count, 3);
        assert_eq!(c.typing, vec!["Bob".to_owned()]);
        assert_eq!(c.preview.unwrap().kind, "m.text");
    }

    #[test]
    fn message_decodes_media_reply_and_reactions() {
        let m: Message = serde_json::from_str(MESSAGE).unwrap();
        assert_eq!(m.kind, "m.image");
        assert_eq!(m.status, MessageStatus::Delivered);
        assert_eq!(m.media.as_ref().unwrap().duration_ms, 0);
        assert_eq!(m.reply_to.as_ref().unwrap().event_id, "$ev0");
        assert_eq!(m.reactions[0].senders.len(), 2);
        assert!(m.live);
    }

    #[test]
    fn null_optionals_and_missing_fields_are_tolerated() {
        let m: Message =
            serde_json::from_str(r##"{"eventId":"$x","media":null,"replyTo":null}"##).unwrap();
        assert!(m.media.is_none() && m.reply_to.is_none());
        assert_eq!(m.order, 0);
        let c: Chat =
            serde_json::from_str(r##"{"roomId":"!a:chirper.local","preview":null}"##).unwrap();
        assert!(c.preview.is_none());
    }

    #[test]
    fn settings_keep_flexible_maps() {
        let s: CoreSettings = serde_json::from_str(SETTINGS).unwrap();
        assert_eq!(s.send_read_receipts["discord"], Value::Bool(false));
        assert_eq!(s.telegram["apiId"], Value::from(0));
        let back: CoreSettings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn chat_filter_and_flags_serialize_like_go() {
        let f = ChatFilter {
            network: "discord".into(),
            unread_only: true,
            ..ChatFilter::default()
        };
        let json = serde_json::to_value(&f).unwrap();
        assert_eq!(json["unreadOnly"], Value::Bool(true));
        assert_eq!(json["archived"], Value::Bool(false));

        let flags = ChatFlags {
            pinned: Some(true),
            ..ChatFlags::default()
        };
        assert_eq!(
            serde_json::to_string(&flags).unwrap(),
            r##"{"pinned":true}"##
        );
    }

    #[test]
    fn core_event_keeps_raw_payload() {
        let ev: CoreEvent =
            serde_json::from_str(r##"{"kind":"message","payload":{"eventId":"$a","order":7}}"##)
                .unwrap();
        assert_eq!(ev.kind, event_kind::MESSAGE);
        assert_eq!(ev.payload.get(), r##"{"eventId":"$a","order":7}"##);
    }
}
