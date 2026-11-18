//! The domain model that crosses the driver boundary.
//!
//! Drivers speak **remote** ids only (a portal id, a message id, a user id). The runtime assigns
//! local ids, so a driver cannot get the identity model wrong, and the engine cannot depend on a
//! network's id format. There is no Matrix type anywhere in this crate.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::ids::{ChatId, EventId, SenderId};

/// What a network can do through Chirper. Mirrors the `capabilities` object of the wire contract.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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

impl Capabilities {
    /// Every capability on: for a network that has not narrowed its feature set.
    pub const ALL: Self = Self {
        edits: true,
        reactions: true,
        replies: true,
        redactions: true,
        typing: true,
        read_receipts: true,
        stickers: true,
        voice: true,
    };

    pub fn with_voice(mut self, value: bool) -> Self {
        self.voice = value;
        self
    }
}

/// One way to log into a network.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LoginFlow {
    pub id: String,
    pub name: String,
    pub description: String,
}

/// A field a login step asks the user to fill in.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct InputField {
    pub id: String,
    pub name: String,
    /// `text`, `password`, `email`, `phone`, `number`, `select`, `button`.
    pub kind: String,
    pub description: String,
    pub options: Vec<String>,
    pub pattern: String,
}

/// Where a cookie or token can be read from a login web page.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CookieSource {
    /// `cookie`, `local_storage`, `session_storage`, `javascript`.
    pub kind: String,
    pub name: String,
    pub cookie_domain: String,
}

/// A cookie or token a login web page must yield.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CookieField {
    pub id: String,
    pub required: bool,
    pub sources: Vec<CookieSource>,
}

/// The next thing a login process needs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum LoginStep {
    /// Ask the user for values.
    UserInput {
        step_id: String,
        instructions: String,
        fields: Vec<InputField>,
    },
    /// Show a web page and collect cookies from it.
    Cookies {
        step_id: String,
        instructions: String,
        url: String,
        user_agent: String,
        fields: Vec<CookieField>,
        extract_js: String,
        wait_for_url_pattern: String,
    },
    /// Show something the user must scan or type.
    DisplayAndWait {
        step_id: String,
        instructions: String,
        /// `qr`, `emoji`, `code`, `nothing`.
        kind: String,
        data: String,
        image_url: String,
    },
    /// The login exists now.
    Complete { login_id: String },
}

impl LoginStep {
    pub fn step_id(&self) -> &str {
        match self {
            Self::UserInput { step_id, .. }
            | Self::Cookies { step_id, .. }
            | Self::DisplayAndWait { step_id, .. } => step_id,
            Self::Complete { .. } => "",
        }
    }

    pub fn is_waiting(&self) -> bool {
        matches!(self, Self::DisplayAndWait { .. })
    }

    pub fn is_complete(&self) -> bool {
        matches!(self, Self::Complete { .. })
    }
}

/// A chat as the network describes it.
#[derive(Clone, Debug, Default)]
pub struct ChatMeta {
    pub portal: PortalRef,
    pub name: String,
    pub topic: String,
    pub avatar_url: String,
    pub is_dm: bool,
    pub is_space: bool,
    pub member_count: i64,
    /// Portal of the parent (a server, guild or workspace), when the network has one.
    pub parent: Option<PortalRef>,
    /// Display name of the parent, when known.
    pub parent_name: String,
    /// A DM partner, when the chat is a direct message.
    pub dm_user: Option<RemoteUser>,
}

/// The remote identity of a chat.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct PortalRef {
    /// The network's own chat id (a room, channel, thread or conversation id).
    pub id: String,
    /// The login part of a portal key; empty unless the same portal exists per account.
    pub receiver: String,
}

/// A person or bot on a network.
#[derive(Clone, Debug, Default)]
pub struct RemoteUser {
    pub id: String,
    pub display_name: String,
    pub avatar_url: String,
    pub username: String,
    pub phone: String,
}

/// One emoji of a message's reaction summary.
#[derive(Clone, Debug, Default)]
pub struct Reaction {
    /// The emoji: the character itself, or `name:id` for a custom emoji.
    pub key: String,
    pub count: i64,
    /// Whether the local user is one of the reactors.
    pub from_me: bool,
    /// Who reacted, when the network says so. Networks that only report counts leave this empty.
    pub senders: Vec<RemoteUser>,
}

/// A member change in a chat.
#[derive(Clone, Debug, Default)]
pub struct Member {
    pub user: RemoteUser,
    /// `join`, `leave`, `invite`, `ban`.
    pub membership: String,
}

/// A locally stored media file, as the engine sees it.
#[derive(Clone, Debug, Default)]
pub struct MediaInfo {
    pub mime: String,
    pub file_name: String,
    pub size: usize,
    pub width: i64,
    pub height: i64,
    pub duration_ms: i64,
    pub blurhash: String,
}

/// What a message holds.
#[derive(Clone, Debug)]
pub enum Content {
    /// Plain text, optionally with the network's own formatting.
    Text {
        body: String,
        formatted: Option<String>,
    },
    /// `/me` style action.
    Emote { body: String },
    /// A bot or system notice.
    Notice { body: String },
    /// An attachment. `data` is the file, already downloaded by the driver.
    Media {
        /// `image`, `video`, `audio`, `voice`, `file`, `sticker`.
        kind: String,
        caption: String,
        info: MediaInfo,
        data: Arc<Vec<u8>>,
        thumbnail: Option<Arc<Vec<u8>>>,
    },
    /// A shared location.
    Location { body: String },
    /// Something Chirper cannot show yet; `body` is the best description available.
    Unsupported { body: String },
}

impl Content {
    pub fn body(&self) -> &str {
        match self {
            Self::Text { body, .. }
            | Self::Emote { body }
            | Self::Notice { body }
            | Self::Location { body }
            | Self::Unsupported { body } => body,
            Self::Media { caption, info, .. } => {
                if caption.is_empty() {
                    &info.file_name
                } else {
                    caption
                }
            }
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Text { .. } => "text",
            Self::Emote { .. } => "emote",
            Self::Notice { .. } => "notice",
            Self::Location { .. } => "location",
            Self::Unsupported { .. } => "unsupported",
            Self::Media { kind, .. } => match kind.as_str() {
                "image" => "image",
                "video" => "video",
                "audio" => "audio",
                "voice" => "voice",
                "sticker" => "sticker",
                _ => "file",
            },
        }
    }
}

/// One message of a chat, as the driver reports it.
#[derive(Clone, Debug)]
pub struct RemoteMessage {
    pub portal: PortalRef,
    /// The network's message id. Empty for a change that has no message of its own (an edit
    /// marker, a redaction) when the target is already known.
    pub id: String,
    /// A part of a multi-part message; empty for the usual single-part message.
    pub part: String,
    pub sender: RemoteUser,
    /// Unix milliseconds on the network's clock, or 0 when unknown.
    pub timestamp_ms: i64,
    pub content: Content,
    /// Remote id of the message this one replies to.
    pub reply_to: Option<String>,
    /// Remote id of the message this one edits.
    pub edit_of: Option<String>,
    /// True when the message arrived in real time and was not part of a history page.
    pub live: bool,
    /// True when the message was sent by the local user from another device.
    pub from_me: bool,
}

/// The verdict of the network on an outgoing event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryStatus {
    /// Accepted by the network's local send queue.
    Sent,
    /// Confirmed by the network.
    Delivered,
    /// Read by the other side, when the network reports it.
    Read,
    /// The network refused it.
    Failed,
}

/// The lifecycle of one login.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    Connecting,
    Backfilling,
    Running,
    /// The saved credentials no longer work and the user must log in again.
    NeedsReauth,
    /// The user logged out.
    LoggedOut,
    /// A temporary network failure; the runtime retries.
    TransientDisconnect,
    /// The credentials were refused.
    BadCredentials,
    UnknownError,
}

impl SessionState {
    /// The `Account.state` value of the wire contract.
    pub fn account_state(self) -> &'static str {
        match self {
            Self::Connecting | Self::Backfilling | Self::Running => "CONNECTED",
            Self::NeedsReauth => "BAD_CREDENTIALS",
            Self::LoggedOut => "LOGGED_OUT",
            Self::TransientDisconnect => "TRANSIENT_DISCONNECT",
            Self::BadCredentials => "BAD_CREDENTIALS",
            Self::UnknownError => "UNKNOWN_ERROR",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::NeedsReauth | Self::LoggedOut | Self::BadCredentials)
    }

    pub fn should_retry(self) -> bool {
        matches!(self, Self::TransientDisconnect | Self::UnknownError)
    }
}

/// What a driver reports. The runtime tags every event with its network and login, assigns local
/// ids, and forwards it to the engine.
#[derive(Debug)]
pub enum Event {
    /// A chat exists or its metadata changed.
    Chat(ChatMeta),
    /// A chat is gone for this account.
    ChatRemoved(PortalRef),
    /// A message arrived, or a redaction of one.
    Message(RemoteMessage),
    /// A message is gone.
    Redact { portal: PortalRef, event: RemoteEventRef },
    /// Reactions on a message changed. The full set replaces the stored one for this message.
    Reactions {
        portal: PortalRef,
        target: RemoteEventRef,
        reactions: Vec<Reaction>,
    },
    /// Someone is typing, or stopped (`senders` empty).
    Typing {
        portal: PortalRef,
        senders: Vec<RemoteUser>,
    },
    /// A membership change.
    Member { portal: PortalRef, member: Member },
    /// The user's read position on another device moved.
    ReadReceipt {
        portal: PortalRef,
        user: RemoteUser,
        event: RemoteEventRef,
    },
    /// The network's verdict on an event this device sent.
    Delivery {
        portal: PortalRef,
        /// The transaction id the engine chose for the send, or the remote id.
        txn_id: String,
        status: DeliveryStatus,
        error: String,
    },
    /// The login's lifecycle moved.
    State {
        state: SessionState,
        error: String,
        message: String,
        /// The remote profile of the logged-in user, when the network knows it.
        profile: Option<RemoteUser>,
    },
    /// A non-fatal failure worth showing as `core_error`.
    Error(String),
}

/// A reference to a remote event: the message it belongs to and the part.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct RemoteEventRef {
    pub id: String,
    pub part: String,
}

impl RemoteEventRef {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            part: String::new(),
        }
    }
}

/// The page size a driver should aim for when its network allows it.
pub const DEFAULT_BACKFILL_PAGE: usize = 50;

/// A page of history.
#[derive(Debug, Default)]
pub struct BackfillPage {
    pub messages: Vec<RemoteMessage>,
    /// Opaque cursor for the next older page; `None` means the beginning of the history.
    pub cursor: Option<String>,
    pub has_more: bool,
}

/// What a driver returns once it accepted a command.
#[derive(Debug, Default)]
pub struct Ack {
    /// The network's id of the created message, when the command created one.
    pub remote_event_id: Option<String>,
}

/// A chat as the engine stores it, after id assignment.
#[derive(Clone, Debug)]
pub struct StoredChat {
    pub chat: ChatId,
    pub portal: PortalRef,
    pub network: String,
    pub login_id: String,
    pub meta: ChatMeta,
}

/// A message as the engine stores it, after id assignment.
#[derive(Clone, Debug)]
pub struct StoredMessage {
    pub chat: ChatId,
    pub portal: PortalRef,
    pub event: EventId,
    pub sender: SenderId,
    pub message: RemoteMessage,
}