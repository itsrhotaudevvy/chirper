//! The JSON of X's messaging endpoints. Every struct defaults its fields, so an answer that leaves
//! one out parses, and an unknown entry type is skipped rather than failing the whole page.
//! Reference: `twittermeow/data/types/*.go` and `data/response/inbox.go`.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

/// A user as the inbox describes them.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct User {
    pub id_str: String,
    pub name: String,
    pub screen_name: String,
    pub profile_image_url_https: String,
}

/// One member of a conversation.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Participant {
    pub user_id: String,
    pub last_read_event_id: String,
}

/// The avatar of a group conversation.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Avatar {
    pub image: AvatarImage,
}

/// The image part of an avatar.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct AvatarImage {
    pub original_info: ImageInfo,
}

/// The address of one image.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ImageInfo {
    pub url: String,
    pub width: i64,
    pub height: i64,
}

/// A conversation of the inbox.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Conversation {
    pub conversation_id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    pub avatar_image_https: String,
    pub avatar: Avatar,
    pub sort_event_id: String,
    pub sort_timestamp: String,
    pub participants: Vec<Participant>,
    pub trusted: bool,
    pub low_quality: bool,
    pub last_read_event_id: String,
}

impl Conversation {
    /// Whether this is a group rather than a one-to-one conversation.
    pub fn is_group(&self) -> bool {
        self.kind == "GROUP_DM"
    }
}

/// The URL entity of a message's text.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct UrlEntity {
    pub url: String,
    pub expanded_url: String,
    pub display_url: String,
    pub indices: Vec<usize>,
}

/// The mention entity of a message's text.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct MentionEntity {
    pub id_str: String,
    pub name: String,
    pub screen_name: String,
    pub indices: Vec<usize>,
}

/// The entities of a message's text.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Entities {
    pub urls: Vec<UrlEntity>,
    pub user_mentions: Vec<MentionEntity>,
    pub media: Vec<MediaInfo>,
}

/// One video variant.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Variant {
    pub bitrate: i64,
    pub content_type: String,
    pub url: String,
}

/// The video part of a media item.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct VideoInfo {
    pub duration_millis: i64,
    pub variants: Vec<Variant>,
}

/// A photo, video, GIF or audio attachment.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct MediaInfo {
    pub id_str: String,
    pub media_url: String,
    pub media_url_https: String,
    pub url: String,
    pub expanded_url: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub filename: String,
    pub filesize_bytes: i64,
    pub original_info: ImageInfo,
    pub video_info: VideoInfo,
    pub audio_only: bool,
    pub media_hash_key: String,
}

/// The attachment slots of a message. A legacy message carries one of these.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Attachment {
    pub photo: Option<MediaInfo>,
    pub video: Option<MediaInfo>,
    pub animated_gif: Option<MediaInfo>,
    pub media: Option<MediaInfo>,
}

/// The reply a message quotes.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ReplyData {
    pub id: String,
}

/// The content of a message.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct MessageData {
    pub id: String,
    pub time: String,
    pub sender_id: String,
    pub text: String,
    pub entities: Option<Entities>,
    pub attachment: Option<Attachment>,
    pub reply_data: ReplyData,
    /// Set on a message of an encrypted (XChat) conversation, whose text is not readable here.
    pub conversation_key_version: String,
}

/// A message of a conversation.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Message {
    pub id: String,
    pub time: String,
    pub conversation_id: String,
    pub request_id: String,
    pub message_data: MessageData,
}

/// A reaction added to or removed from a message.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Reaction {
    pub id: String,
    pub time: String,
    pub conversation_id: String,
    pub message_id: String,
    pub emoji_reaction: String,
    pub sender_id: String,
}

/// A read marker of a conversation.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ConversationRead {
    pub id: String,
    pub time: String,
    pub conversation_id: String,
    pub last_read_event_id: String,
    pub sender_id: String,
}

/// A conversation that was removed for this account.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Removal {
    pub id: String,
    pub time: String,
    pub conversation_id: String,
}

/// A membership change of a conversation.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Membership {
    pub id: String,
    pub time: String,
    pub conversation_id: String,
    pub sender_id: String,
    pub participants: Vec<Participant>,
}

/// One entry of an inbox page, after its type is known.
#[derive(Clone, Debug)]
pub enum Entry {
    Message(Box<Message>),
    ReactionCreate(Reaction),
    ReactionDelete(Reaction),
    ConversationRead(ConversationRead),
    ConversationRemoved(Removal),
    ParticipantsJoin(Membership),
    ParticipantsLeave(Membership),
    /// An entry type this driver does not use yet.
    Other(String),
}

/// A page of the inbox or of one conversation.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Inbox {
    pub cursor: String,
    pub min_entry_id: String,
    pub max_entry_id: String,
    pub status: String,
    pub entries: Vec<Value>,
    pub users: HashMap<String, User>,
    pub conversations: HashMap<String, Conversation>,
}

impl Inbox {
    /// The entries of this page, each parsed by its type. Unknown entries are kept as `Other`.
    pub fn parsed_entries(&self) -> Vec<Entry> {
        self.entries.iter().filter_map(parse_entry).collect()
    }
}

/// Parse one entry. The entry's type is the one key it carries; a value that does not match its
/// type's shape is dropped with `None`.
pub fn parse_entry(value: &Value) -> Option<Entry> {
    let object = value.as_object()?;
    let (key, inner) = object.iter().next()?;
    let entry = match key.as_str() {
        "message" => Entry::Message(Box::new(decode(inner)?)),
        "reaction_create" => Entry::ReactionCreate(decode(inner)?),
        "reaction_delete" => Entry::ReactionDelete(decode(inner)?),
        "conversation_read" => Entry::ConversationRead(decode(inner)?),
        "remove_conversation" => Entry::ConversationRemoved(decode(inner)?),
        "participants_join" => Entry::ParticipantsJoin(decode(inner)?),
        "participants_leave" => Entry::ParticipantsLeave(decode(inner)?),
        other => Entry::Other(other.to_owned()),
    };
    Some(entry)
}

fn decode<T: for<'de> Deserialize<'de>>(value: &Value) -> Option<T> {
    serde_json::from_value(value.clone()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_message_entry_parses_with_its_content() {
        let entry = json!({"message": {
            "id": "1001", "time": "1700000000000", "conversation_id": "111-222",
            "message_data": {"id": "5", "text": "hi", "sender_id": "222",
                "reply_data": {"id": "900"}}
        }});
        let Some(Entry::Message(message)) = parse_entry(&entry) else {
            panic!("expected a message entry");
        };
        assert_eq!(message.id, "1001");
        assert_eq!(message.conversation_id, "111-222");
        assert_eq!(message.message_data.text, "hi");
        assert_eq!(message.message_data.reply_data.id, "900");
    }

    #[test]
    fn unknown_entries_are_kept_by_name_and_junk_is_dropped() {
        assert!(matches!(
            parse_entry(&json!({"trust_conversation": {}})),
            Some(Entry::Other(name)) if name == "trust_conversation"
        ));
        assert!(parse_entry(&json!("not an object")).is_none());
        assert!(parse_entry(&json!({})).is_none());
    }

    #[test]
    fn a_page_tolerates_missing_fields() {
        let page: Inbox = serde_json::from_value(json!({"cursor": "c1"})).unwrap();
        assert_eq!(page.cursor, "c1");
        assert!(page.entries.is_empty());
    }
}
