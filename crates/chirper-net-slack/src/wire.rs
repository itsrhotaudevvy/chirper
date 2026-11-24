//! Slack Web API and Socket (RTM) payloads. Every field is optional on the wire, so every struct
//! defaults; the mapping decides what a missing field means.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

/// `auth.test`: the team and the user a token belongs to.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AuthTest {
    pub url: String,
    pub team: String,
    pub team_id: String,
    pub user: String,
    pub user_id: String,
}

/// `rtm.connect`: the websocket to open.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RtmConnect {
    pub url: String,
}

/// The cursor a paged list answer carries.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ResponseMetadata {
    pub next_cursor: String,
}

/// `conversations.list`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ConversationsList {
    pub channels: Vec<Channel>,
    pub response_metadata: ResponseMetadata,
}

/// `users.list`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct UsersList {
    pub members: Vec<User>,
    pub response_metadata: ResponseMetadata,
}

/// `users.info`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct UserInfo {
    pub user: User,
}

/// `bots.info`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BotInfo {
    pub bot: Bot,
}

/// `conversations.history` and `conversations.replies`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct History {
    pub messages: Vec<Message>,
    pub has_more: bool,
}

/// `chat.postMessage`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Posted {
    pub ts: String,
}

/// `reactions.get`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ReactionsGet {
    pub message: Message,
}

/// `files.getUploadURLExternal`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct UploadUrl {
    pub upload_url: String,
    pub file_id: String,
}

/// `files.completeUploadExternal`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CompletedUpload {
    pub files: Vec<File>,
}

/// A channel, group, DM or group DM from `conversations.list` or a channel event.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub is_channel: bool,
    pub is_group: bool,
    pub is_im: bool,
    pub is_mpim: bool,
    pub is_private: bool,
    pub is_member: bool,
    pub is_archived: bool,
    /// The other person of a DM.
    pub user: String,
    pub num_members: i64,
    pub topic: Topic,
    pub purpose: Topic,
    pub created: i64,
    pub updated: i64,
}

/// A Slack `{value}` text field.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Topic {
    pub value: String,
}

/// A user account.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct User {
    pub id: String,
    pub name: String,
    pub real_name: String,
    pub deleted: bool,
    pub is_bot: bool,
    pub is_app_user: bool,
    pub profile: Profile,
}

/// The profile block of a user.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub display_name: String,
    pub real_name: String,
    pub image_512: String,
}

/// A bot user from `bots.info`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Bot {
    pub id: String,
    pub name: String,
    pub deleted: bool,
}

/// A message: from a history page, a `message` event, or the `message` of an edit event.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Message {
    pub ts: String,
    pub user: String,
    pub bot_id: String,
    pub username: String,
    pub text: String,
    pub subtype: String,
    pub thread_ts: String,
    pub blocks: Vec<Value>,
    pub files: Vec<File>,
    pub reactions: Vec<Reaction>,
    pub hidden: bool,
    /// Event envelope only: the conversation the message is in.
    pub channel: String,
    /// Event envelope only: `channel`, `group`, `im` or `mpim`.
    pub channel_type: String,
    /// Event envelope only: when the event happened.
    pub event_ts: String,
    /// `message_deleted`: the timestamp of the message that was removed.
    pub deleted_ts: String,
    /// `message_changed`: the message as it is now.
    pub message: Option<Box<Message>>,
}

/// A file attached to a message.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct File {
    pub id: String,
    pub name: String,
    pub title: String,
    pub mimetype: String,
    pub filetype: String,
    pub subtype: String,
    pub mode: String,
    pub size: u64,
    pub url_private: String,
    pub url_private_download: String,
    pub is_external: bool,
    pub original_w: i64,
    pub original_h: i64,
    pub duration_ms: i64,
    pub shares: Option<Shares>,
}

/// Where a file was shared, by conversation id.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Shares {
    pub public: HashMap<String, Vec<Share>>,
    pub private: HashMap<String, Vec<Share>>,
}

/// One share of a file in a conversation: the timestamp of the message that carries it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Share {
    pub ts: String,
}

/// One reaction of a message: the emoji name, its count and the users who used it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Reaction {
    pub name: String,
    pub count: i64,
    pub users: Vec<String>,
}

/// A workspace a sign-in address can reach, as `signin.findWorkspaces` names it.
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub url: String,
    pub domain: String,
    /// A one-time link that signs the browser into the workspace.
    pub magic_login_url: String,
    /// A one-time code that signs the browser into the workspace.
    pub magic_login_code: String,
    pub sso_required: bool,
    pub two_factor_required: bool,
}

impl std::fmt::Debug for Workspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Workspace")
            .field("id", &self.id)
            .field("magic_login", &"[redacted]")
            .finish_non_exhaustive()
    }
}
