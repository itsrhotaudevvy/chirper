//! Google Messages objects → Chirper's domain model: conversations to chats, messages to parts,
//! reactions, deletions and typing. Port of `connector/chatinfo.go`, `handlegmessages.go` and
//! `messagestatus.go`; the rules are the Go connector's, the output is bridge events.
//!
//! Attachments are described first ([`Part::Media`]) and downloaded by the session, which is why the
//! conversion itself is synchronous.

use chirper_bridge::model::{ChatMeta, Content, Reaction, RemoteUser};

use crate::ids;
use crate::proto::chat::{Conversation, Message, MessageInfo, Participant, ReactionEntry, message_info};

/// `MessageStatusType` values the conversion looks at.
pub mod status {
    pub const OUTGOING_COMPLETE: i32 = 1;
    pub const OUTGOING_MAX: i32 = 99;
    pub const INCOMING_MIN: i32 = 100;
    pub const INCOMING_MAX: i32 = 199;
    pub const INCOMING_YET_TO_MANUAL_DOWNLOAD: i32 = 101;
    pub const INCOMING_MANUAL_DOWNLOADING: i32 = 103;
    pub const INCOMING_AUTO_DOWNLOADING: i32 = 105;
    pub const INCOMING_DOWNLOAD_FAILED: i32 = 106;
    pub const INCOMING_RETRYING_AUTO_DOWNLOAD: i32 = 104;
    pub const INCOMING_RETRYING_MANUAL_DOWNLOAD: i32 = 102;
    pub const INCOMING_DOWNLOAD_CANCELED: i32 = 110;
    pub const INCOMING_DOWNLOAD_FAILED_TOO_LARGE: i32 = 111;
    pub const INCOMING_DOWNLOAD_FAILED_SIM_HAS_NO_DATA: i32 = 112;
    pub const MESSAGE_DELETED: i32 = 300;
    pub const TOMBSTONE_MIN: i32 = 200;
    pub const TOMBSTONE_MAX: i32 = 299;
}

/// The identity a conversion runs under.
#[derive(Debug, Clone, Copy)]
pub struct Scope<'a> {
    /// The account prefix of the ids.
    pub prefix: &'a str,
    /// The login id (the portal key's receiver).
    pub login_id: &'a str,
    /// The participant id of the local user, when the conversation names it.
    pub me: Option<&'a str>,
}

/// A media attachment, before its bytes are fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaSpec {
    /// `image`, `video`, `audio`, `file`.
    pub kind: String,
    pub file_name: String,
    pub mime: String,
    pub size: i64,
    pub width: i64,
    pub height: i64,
    /// The attachment's Google id, the key of its download.
    pub media_id: String,
    pub key: Vec<u8>,
    /// Used when the full attachment is not there yet.
    pub thumbnail_id: String,
    pub thumbnail_key: Vec<u8>,
    /// The bytes the message carries itself, when present.
    pub inline: Vec<u8>,
}

/// One part of a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    /// Text, with the subject and the bodies of the message.
    Text {
        body: String,
        formatted: Option<String>,
    },
    /// A notice that stands in for content not here yet (a download in progress, a failure).
    Notice { body: String },
    /// An attachment, to be fetched by the session.
    Media {
        /// The part id: the action message id, or empty for the first part.
        part: String,
        spec: MediaSpec,
    },
}

/// Whether a message is a bridged message at all.
pub fn is_bridged(status: i32) -> bool {
    !(status == status::MESSAGE_DELETED || (status::TOMBSTONE_MIN..=status::TOMBSTONE_MAX).contains(&status))
}

/// The text of the status a download is in, when there is one (`downloadPendingStatusMessage`).
pub fn download_status_text(status: i32) -> &'static str {
    match status {
        status::INCOMING_YET_TO_MANUAL_DOWNLOAD => {
            "Attachment message (auto-download is disabled, use Messages on Android to download)"
        }
        status::INCOMING_MANUAL_DOWNLOADING
        | status::INCOMING_AUTO_DOWNLOADING
        | status::INCOMING_RETRYING_MANUAL_DOWNLOAD
        | status::INCOMING_RETRYING_AUTO_DOWNLOAD => "Downloading message...",
        status::INCOMING_DOWNLOAD_FAILED => "Message download failed",
        status::INCOMING_DOWNLOAD_FAILED_TOO_LARGE => "Message download failed (too large)",
        status::INCOMING_DOWNLOAD_FAILED_SIM_HAS_NO_DATA => {
            "Message download failed (no mobile data connection)"
        }
        status::INCOMING_DOWNLOAD_CANCELED => "Message download canceled",
        _ => "",
    }
}

/// Who sent a message and whether the local user did.
pub fn sender_of(scope: Scope<'_>, message: &Message) -> (RemoteUser, bool) {
    let status = message_status(message);
    let participant = message.participant_id.as_str();
    let outgoing = (status::OUTGOING_COMPLETE..=status::OUTGOING_MAX).contains(&status);
    let incoming = (status::INCOMING_MIN..=status::INCOMING_MAX).contains(&status);
    let from_me = outgoing || (!incoming && scope.me.is_some_and(|me| me == participant));
    let sender = RemoteUser {
        id: ids::user(scope.prefix, participant),
        display_name: String::new(),
        avatar_url: String::new(),
        username: String::new(),
        phone: String::new(),
    };
    (sender, from_me)
}

fn message_status(message: &Message) -> i32 {
    message.message_status.as_ref().map(|s| s.status).unwrap_or(0)
}

/// The parts of a message: its text (subject and bodies), then each attachment.
pub fn parts_of(message: &Message) -> Vec<Part> {
    let status = message_status(message);
    let mut body = String::new();
    let mut formatted = String::new();
    let subject = message.subject.clone().unwrap_or_default();
    if !subject.is_empty() {
        body = format!("**{subject}**");
        formatted = format!("<strong>{}</strong>", escape(&subject));
    }
    let mut attachments = Vec::new();
    let mut first_part_taken = false;
    for info in &message.message_info {
        match &info.data {
            Some(message_info::Data::MessageContent(content)) => {
                push_text(&mut body, &mut formatted, &content.content, !subject.is_empty());
            }
            Some(message_info::Data::LocationContent(location)) => {
                if let Some(content) = &location.message_content {
                    push_text(&mut body, &mut formatted, &content.content, !subject.is_empty());
                }
            }
            Some(message_info::Data::MediaContent(media)) => {
                attachments.push((info, media));
            }
            None => {}
        }
    }
    let mut parts = Vec::new();
    let status_text = download_status_text(status);
    let mut text_body = body.clone();
    let mut text_formatted = (!formatted.is_empty()).then(|| formatted.clone());
    if !status_text.is_empty() {
        if text_body.is_empty() {
            text_body = status_text.to_owned();
        } else {
            text_body = format!("{text_body}\n\n{status_text}");
            text_formatted = text_formatted.map(|f| format!("<p>{f}</p><p>{}</p>", escape(status_text)));
        }
    }
    if !text_body.is_empty() {
        parts.push(Part::Text {
            body: text_body,
            formatted: text_formatted,
        });
        first_part_taken = true;
    }
    for (info, media) in attachments {
        let spec = media_spec(media);
        let part = if first_part_taken {
            info.action_message_id.clone().unwrap_or_default()
        } else {
            String::new()
        };
        first_part_taken = true;
        parts.push(Part::Media { part, spec });
    }
    parts
}

fn push_text(body: &mut String, formatted: &mut String, text: &str, has_subject: bool) {
    if !body.is_empty() {
        body.push('\n');
    }
    body.push_str(text);
    if has_subject || !formatted.is_empty() {
        if !formatted.is_empty() {
            formatted.push_str("<br>");
        }
        formatted.push_str(&escape(text));
    }
}

/// Minimal HTML escaping for the text the bridge sends as `formatted_body`.
pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn media_spec(media: &crate::proto::chat::MediaContent) -> MediaSpec {
    let mime = if media.mime_type.is_empty() {
        "application/octet-stream".to_owned()
    } else {
        media.mime_type.clone()
    };
    let kind = match mime.split('/').next().unwrap_or_default() {
        "image" => "image",
        "video" => "video",
        "audio" => "audio",
        _ => "file",
    };
    let (width, height) = media
        .dimensions
        .map(|d| (d.width, d.height))
        .unwrap_or((0, 0));
    MediaSpec {
        kind: kind.to_owned(),
        file_name: media.media_name.clone(),
        mime,
        size: media.size,
        width,
        height,
        media_id: media.media_id.clone(),
        key: media.decryption_key.clone(),
        thumbnail_id: media.thumbnail_media_id.clone(),
        thumbnail_key: media.thumbnail_decryption_key.clone(),
        inline: media.media_data.clone(),
    }
}

/// The notice that stands in for an attachment the driver could not fetch.
pub fn attachment_notice(spec: &MediaSpec, failed: bool) -> Content {
    let body = if failed {
        format!("Failed to transfer attachment {}", spec.file_name)
    } else {
        format!("Waiting for attachment {}", spec.file_name)
    };
    Content::Notice { body }
}

/// Turn a text or notice part into content.
pub fn text_content(part: &Part) -> Option<Content> {
    match part {
        Part::Text { body, formatted } => Some(Content::Text {
            body: body.clone(),
            formatted: formatted.clone(),
        }),
        Part::Notice { body } => Some(Content::Notice { body: body.clone() }),
        Part::Media { .. } => None,
    }
}

/// The reactions of a message, grouped by emoji. The full set replaces the stored one.
pub fn reactions_of(scope: Scope<'_>, entries: &[ReactionEntry]) -> Vec<Reaction> {
    let mut out: Vec<Reaction> = Vec::new();
    for entry in entries {
        let Some(data) = entry.data.as_ref() else {
            continue;
        };
        let key = match data.r#type {
            13 => ":custom:".to_owned(),
            8 => data.unicode.clone(),
            other => match emoji_for_type(other) {
                Some(emoji) => emoji.to_owned(),
                None => continue,
            },
        };
        if key.is_empty() {
            continue;
        }
        let senders: Vec<RemoteUser> = entry
            .participant_ids
            .iter()
            .map(|id| RemoteUser {
                id: ids::user(scope.prefix, id),
                ..Default::default()
            })
            .collect();
        let from_me = scope
            .me
            .is_some_and(|me| entry.participant_ids.iter().any(|id| id == me));
        let count = i64::try_from(entry.participant_ids.len()).unwrap_or(i64::MAX);
        match out.iter_mut().find(|reaction| reaction.key == key) {
            Some(existing) => {
                existing.count += count;
                existing.from_me |= from_me;
                existing.senders.extend(senders);
            }
            None => out.push(Reaction {
                key,
                count,
                from_me,
                senders,
            }),
        }
    }
    out
}

/// The emoji of an `EmojiType` value (`emojitype.go`).
pub fn emoji_for_type(kind: i32) -> Option<&'static str> {
    Some(match kind {
        1 => "👍",
        2 => "😍",
        3 => "😂",
        4 => "😮",
        5 => "😥",
        6 => "😠",
        7 => "👎",
        9 => "🤔",
        10 => "😢",
        11 => "😡",
        12 => "❤️",
        _ => return None,
    })
}

/// The `EmojiType` of an emoji sent by the user (`UnicodeToEmojiType`), 8 for anything else.
pub fn type_for_emoji(emoji: &str) -> i32 {
    match emoji {
        "👍" => 1,
        "😍" => 2,
        "😂" => 3,
        "😮" => 4,
        "😥" => 5,
        "😠" => 6,
        "👎" => 7,
        "🤔" => 9,
        "😢" => 10,
        "😡" => 11,
        "❤" | "❤️" => 12,
        _ => 8,
    }
}

/// The chat a conversation names.
pub fn chat_meta(scope: Scope<'_>, conversation: &Conversation) -> ChatMeta {
    let portal = ids::portal(scope.prefix, scope.login_id, &conversation.conversation_id);
    let is_dm = !conversation.is_group_chat;
    let others: Vec<&Participant> = conversation
        .participants
        .iter()
        .filter(|p| !p.is_me)
        .collect();
    let name = if conversation.name.is_empty() {
        others
            .iter()
            .map(|p| participant_name(p))
            .filter(|n| !n.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        conversation.name.clone()
    };
    let dm_user = if is_dm {
        others.first().map(|p| participant_user(scope, p))
    } else {
        None
    };
    ChatMeta {
        portal,
        name,
        topic: String::new(),
        avatar_url: conversation.group_avatar_url.clone(),
        is_dm,
        is_space: false,
        member_count: i64::try_from(conversation.participants.len()).unwrap_or(0),
        parent: None,
        parent_name: String::new(),
        dm_user,
    }
}

/// A participant as a remote person.
pub fn participant_user(scope: Scope<'_>, participant: &Participant) -> RemoteUser {
    let id = participant
        .id
        .as_ref()
        .map(|info| info.participant_id.clone())
        .unwrap_or_default();
    RemoteUser {
        id: ids::user(scope.prefix, &id),
        display_name: participant_name(participant),
        avatar_url: String::new(),
        username: String::new(),
        phone: participant
            .id
            .as_ref()
            .map(|info| info.number.clone())
            .unwrap_or_default(),
    }
}

/// The name a participant is shown with: full name, then first name, then the number.
pub fn participant_name(participant: &Participant) -> String {
    if !participant.full_name.is_empty() {
        return participant.full_name.clone();
    }
    if !participant.first_name.is_empty() {
        return participant.first_name.clone();
    }
    participant
        .id
        .as_ref()
        .map(|info| info.number.clone())
        .unwrap_or_default()
}

/// The participant id a phone number belongs to in a conversation (for typing events).
pub fn participant_for_number<'a>(conversation: &'a Conversation, number: &str) -> Option<&'a str> {
    conversation
        .participants
        .iter()
        .find(|p| p.id.as_ref().is_some_and(|info| info.number == number))
        .and_then(|p| p.id.as_ref())
        .map(|info| info.participant_id.as_str())
        .filter(|id| !id.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::chat::{MediaContent, MessageContent, MessageStatus};

    fn scope() -> Scope<'static> {
        Scope {
            prefix: "0123456789abcdef",
            login_id: "login",
            me: Some("me-1"),
        }
    }

    fn text_message(status: i32, text: &str) -> Message {
        Message {
            message_id: "m1".to_owned(),
            participant_id: "p1".to_owned(),
            message_status: Some(MessageStatus {
                status,
                ..Default::default()
            }),
            message_info: vec![MessageInfo {
                action_message_id: None,
                data: Some(message_info::Data::MessageContent(MessageContent {
                    content: text.to_owned(),
                })),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn a_text_message_is_one_text_part() {
        let parts = parts_of(&text_message(100, "hello <b>"));
        assert_eq!(
            parts,
            vec![Part::Text {
                body: "hello <b>".to_owned(),
                formatted: None
            }]
        );
    }

    #[test]
    fn a_subject_is_bold_in_the_body_and_strong_in_the_html() {
        let mut message = text_message(100, "body");
        message.subject = Some("Plans".to_owned());
        let parts = parts_of(&message);
        assert_eq!(
            parts,
            vec![Part::Text {
                body: "**Plans**\nbody".to_owned(),
                formatted: Some("<strong>Plans</strong><br>body".to_owned()),
            }]
        );
    }

    #[test]
    fn outgoing_statuses_are_from_me_and_incoming_are_not() {
        let (_, mine) = sender_of(scope(), &text_message(1, "x"));
        assert!(mine);
        let (sender, theirs) = sender_of(scope(), &text_message(100, "x"));
        assert!(!theirs);
        assert_eq!(sender.id, "0123456789abcdef.p1");
    }

    #[test]
    fn deleted_and_tombstone_statuses_are_not_bridged_as_messages() {
        assert!(!is_bridged(status::MESSAGE_DELETED));
        assert!(!is_bridged(230));
        assert!(is_bridged(100));
    }

    #[test]
    fn a_download_in_progress_is_shown_as_text() {
        let mut message = text_message(status::INCOMING_AUTO_DOWNLOADING, "");
        message.message_info.clear();
        assert_eq!(
            parts_of(&message),
            vec![Part::Text {
                body: "Downloading message...".to_owned(),
                formatted: None
            }]
        );
    }

    #[test]
    fn attachments_after_text_carry_their_action_message_id() {
        let mut message = text_message(100, "look");
        message.message_info.push(MessageInfo {
            action_message_id: Some("act-2".to_owned()),
            data: Some(message_info::Data::MediaContent(MediaContent {
                media_id: "media-1".to_owned(),
                media_name: "pic.jpg".to_owned(),
                mime_type: "image/jpeg".to_owned(),
                size: 9,
                ..Default::default()
            })),
        });
        let parts = parts_of(&message);
        assert_eq!(parts.len(), 2);
        match &parts[1] {
            Part::Media { part, spec } => {
                assert_eq!(part, "act-2");
                assert_eq!(spec.kind, "image");
                assert_eq!(spec.media_id, "media-1");
            }
            other => panic!("expected media, got {other:?}"),
        }
    }

    #[test]
    fn reactions_group_by_emoji_and_mark_the_local_user() {
        let entries = vec![
            ReactionEntry {
                data: Some(crate::proto::chat::ReactionData {
                    unicode: String::new(),
                    r#type: 1,
                }),
                participant_ids: vec!["me-1".to_owned(), "p2".to_owned()],
            },
            ReactionEntry {
                data: Some(crate::proto::chat::ReactionData {
                    unicode: "🎉".to_owned(),
                    r#type: 8,
                }),
                participant_ids: vec!["p3".to_owned()],
            },
        ];
        let reactions = reactions_of(scope(), &entries);
        assert_eq!(reactions.len(), 2);
        assert_eq!(reactions[0].key, "👍");
        assert_eq!(reactions[0].count, 2);
        assert!(reactions[0].from_me);
        assert_eq!(reactions[1].key, "🎉");
        assert!(!reactions[1].from_me);
    }

    #[test]
    fn emoji_types_round_trip_for_the_common_reactions() {
        for emoji in ["👍", "😍", "😂", "❤️"] {
            assert_eq!(emoji_for_type(type_for_emoji(emoji)), Some(emoji));
        }
        assert_eq!(type_for_emoji("🎉"), 8);
    }

    #[test]
    fn a_group_chat_is_named_by_its_participants_when_unnamed() {
        let conversation = Conversation {
            conversation_id: "c1".to_owned(),
            is_group_chat: true,
            participants: vec![
                Participant {
                    full_name: "Ada".to_owned(),
                    is_me: false,
                    ..Default::default()
                },
                Participant {
                    first_name: "Grace".to_owned(),
                    is_me: false,
                    ..Default::default()
                },
                Participant {
                    is_me: true,
                    full_name: "Me".to_owned(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let meta = chat_meta(scope(), &conversation);
        assert_eq!(meta.name, "Ada, Grace");
        assert!(!meta.is_dm);
        assert_eq!(meta.portal.id, "0123456789abcdef.c1");
        assert_eq!(meta.portal.receiver, "login");
    }

    #[test]
    fn a_direct_chat_names_its_other_person() {
        let conversation = Conversation {
            conversation_id: "c2".to_owned(),
            participants: vec![Participant {
                full_name: "Linus".to_owned(),
                id: Some(crate::proto::chat::SmallInfo {
                    number: "+15550100".to_owned(),
                    participant_id: "p9".to_owned(),
                    ..Default::default()
                }),
                ..Default::default()
            }],
            ..Default::default()
        };
        let meta = chat_meta(scope(), &conversation);
        assert!(meta.is_dm);
        assert_eq!(meta.name, "Linus");
        let dm = meta.dm_user.expect("a DM partner");
        assert_eq!(dm.id, "0123456789abcdef.p9");
        assert_eq!(dm.phone, "+15550100");
    }
}
