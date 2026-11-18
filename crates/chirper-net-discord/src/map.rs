//! Discord payloads and their mapping onto the bridge's domain types.

use serde::Deserialize;

use chirper_bridge::model::{ChatMeta, Content, MediaInfo, PortalRef, Reaction, RemoteMessage, RemoteUser};

use crate::api::{Api, CDN};

/// Largest attachment that is downloaded into the local media store.
pub const MAX_ATTACHMENT: usize = 24 << 20;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct User {
    pub id: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub global_name: Option<String>,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub bot: bool,
}

/// The fields below mirror Discord's payloads; some are kept for completeness even when the
/// mapping does not read them.
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Attachment {
    pub id: String,
    pub filename: String,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub size: i64,
    #[serde(default)]
    pub width: Option<i64>,
    #[serde(default)]
    pub height: Option<i64>,
    pub url: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Emoji {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ReactionCount {
    pub emoji: Emoji,
    #[serde(default)]
    pub count: i64,
    #[serde(default)]
    pub me: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct MessageReference {
    #[serde(default)]
    pub message_id: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Message {
    pub id: String,
    pub channel_id: String,
    #[serde(default)]
    pub guild_id: Option<String>,
    pub author: User,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub edited_timestamp: Option<String>,
    #[serde(default)]
    pub message_reference: Option<MessageReference>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub reactions: Vec<ReactionCount>,
    #[serde(default)]
    pub sticker_items: Vec<serde_json::Value>,
    #[serde(default)]
    pub embeds: Vec<serde_json::Value>,
    #[serde(rename = "type", default)]
    pub kind: i64,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Channel {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: i64,
    #[serde(default)]
    pub guild_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub recipients: Vec<User>,
    #[serde(default)]
    pub position: i64,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Guild {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub unavailable: bool,
}

/// The display name of a Discord user.
pub fn display_name(user: &User) -> String {
    match &user.global_name {
        Some(name) if !name.trim().is_empty() => name.clone(),
        _ if !user.username.is_empty() => user.username.clone(),
        _ => user.id.clone(),
    }
}

pub fn avatar_url(user: &User) -> String {
    match &user.avatar {
        Some(hash) if !hash.is_empty() => format!("{CDN}/avatars/{}/{hash}.png", user.id),
        _ => String::new(),
    }
}

pub fn guild_avatar(guild: &Guild) -> String {
    match &guild.icon {
        Some(hash) if !hash.is_empty() => format!("{CDN}/icons/{}/{hash}.png", guild.id),
        _ => String::new(),
    }
}

pub fn to_remote_user(user: &User) -> RemoteUser {
    RemoteUser {
        id: user.id.clone(),
        display_name: display_name(user),
        avatar_url: avatar_url(user),
        username: user.username.clone(),
        phone: String::new(),
    }
}

/// A guild channel, DM or group DM as a chat. Voice channels and categories are not chats.
pub fn chat_meta(channel: &Channel, guild_name: &str) -> Option<ChatMeta> {
    let portal = PortalRef {
        id: channel.id.clone(),
        receiver: String::new(),
    };
    let guild = channel.guild_id.clone().filter(|id| !id.is_empty());
    match channel.kind {
        // Guild text, announcement, forum and thread channels.
        0 | 5 | 10 | 11 | 12 | 15 => Some(ChatMeta {
            portal,
            name: channel.name.clone().unwrap_or_else(|| channel.id.clone()),
            topic: channel.topic.clone().unwrap_or_default(),
            avatar_url: String::new(),
            is_dm: false,
            is_space: false,
            member_count: 0,
            parent: guild.clone().map(|id| PortalRef {
                id,
                receiver: String::new(),
            }),
            parent_name: guild_name.to_owned(),
            dm_user: None,
        }),
        // Direct message.
        1 => {
            let partner = channel.recipients.first();
            Some(ChatMeta {
                portal,
                name: partner
                    .map(display_name)
                    .unwrap_or_else(|| "Direct message".to_owned()),
                topic: String::new(),
                avatar_url: partner.map(avatar_url).unwrap_or_default(),
                is_dm: true,
                is_space: false,
                member_count: 2,
                parent: None,
                parent_name: String::new(),
                dm_user: partner.map(to_remote_user),
            })
        }
        // Group DM.
        3 => Some(ChatMeta {
            portal,
            name: channel
                .name
                .clone()
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| {
                    channel
                        .recipients
                        .iter()
                        .map(display_name)
                        .collect::<Vec<_>>()
                        .join(", ")
                }),
            topic: String::new(),
            avatar_url: String::new(),
            is_dm: false,
            is_space: false,
            member_count: channel.recipients.len() as i64 + 1,
            parent: None,
            parent_name: String::new(),
            dm_user: None,
        }),
        _ => None,
    }
}

/// A guild as a space chat, the parent of its channels.
pub fn guild_meta(guild: &Guild) -> ChatMeta {
    ChatMeta {
        portal: PortalRef {
            id: guild.id.clone(),
            receiver: String::new(),
        },
        name: if guild.name.is_empty() {
            guild.id.clone()
        } else {
            guild.name.clone()
        },
        topic: String::new(),
        avatar_url: guild_avatar(guild),
        is_dm: false,
        is_space: true,
        member_count: 0,
        parent: None,
        parent_name: String::new(),
        dm_user: None,
    }
}

/// The reaction summary of a message.
pub fn reactions(message: &Message) -> Vec<Reaction> {
    message
        .reactions
        .iter()
        .map(|reaction| Reaction {
            key: emoji_key(&reaction.emoji),
            count: reaction.count,
            from_me: reaction.me,
            senders: Vec::new(),
        })
        .collect()
}

fn emoji_key(emoji: &Emoji) -> String {
    match (&emoji.id, &emoji.name) {
        (Some(id), Some(name)) if !name.is_empty() => format!("{name}:{id}"),
        (Some(id), _) => id.clone(),
        (None, Some(name)) => name.clone(),
        (None, None) => "?".to_owned(),
    }
}

/// Convert one message, downloading its first attachment when there is one.
pub async fn to_remote_message(api: &Api, message: &Message, live: bool) -> RemoteMessage {
    let mut content = text_content(&message.content);
    if let Some(attachment) = message.attachments.first() {
        let mime = attachment
            .content_type
            .clone()
            .unwrap_or_else(|| "application/octet-stream".to_owned());
        let kind = media_kind(&mime);
        let oversized = attachment.size > MAX_ATTACHMENT as i64;
        let downloaded = if oversized {
            Err(chirper_bridge::Error::Protocol("attachment is too large".to_owned()))
        } else {
            api.download(&attachment.url, MAX_ATTACHMENT).await
        };
        match downloaded {
            Ok(data) if !data.is_empty() => {
                content = Content::Media {
                    kind: kind.to_owned(),
                    caption: if message.content.is_empty() {
                        attachment
                            .description
                            .clone()
                            .unwrap_or_default()
                    } else {
                        message.content.clone()
                    },
                    info: MediaInfo {
                        mime,
                        file_name: attachment.filename.clone(),
                        size: data.len(),
                        width: attachment.width.unwrap_or(0),
                        height: attachment.height.unwrap_or(0),
                        duration_ms: 0,
                        blurhash: String::new(),
                    },
                    data: std::sync::Arc::new(data),
                    thumbnail: None,
                };
            }
            _ => {
                content = Content::Text {
                    body: format!("📎 {}", attachment.filename),
                    formatted: None,
                };
            }
        }
        let _ = attachment.id;
    } else if let Some(sticker) = message.sticker_items.first() {
        let name = sticker
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("sticker");
        content = Content::Text {
            body: format!("Sticker: {name}"),
            formatted: None,
        };
    } else if message.content.trim().is_empty() {
        if let Some(embed) = message.embeds.first() {
            let title = embed.get("title").and_then(|t| t.as_str()).unwrap_or("");
            let description = embed
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("");
            let body = if !title.is_empty() {
                format!("{title}\n{description}")
            } else {
                description.to_owned()
            };
            if !body.trim().is_empty() {
                content = Content::Text {
                    body: body.trim().to_owned(),
                    formatted: None,
                };
            }
        }
    }

    RemoteMessage {
        portal: PortalRef {
            id: message.channel_id.clone(),
            receiver: String::new(),
        },
        id: message.id.clone(),
        part: String::new(),
        sender: to_remote_user(&message.author),
        timestamp_ms: ts_ms(&message.timestamp),
        content,
        reply_to: message
            .message_reference
            .as_ref()
            .and_then(|r| r.message_id.clone())
            .filter(|id| !id.is_empty()),
        edit_of: edited(message).then(|| message.id.clone()),
        live,
        from_me: false,
    }
}

/// Pins, joins, boosts and calls are not conversation and never become messages.
pub fn is_system(message: &Message) -> bool {
    matches!(message.kind, 6 | 7 | 8 | 9 | 10 | 11 | 12)
}

fn text_content(body: &str) -> Content {
    if body.starts_with("*") && body.ends_with('*') && body.len() > 2 {
        Content::Emote {
            body: body.trim_matches('*').to_owned(),
        }
    } else {
        Content::Text {
            body: body.to_owned(),
            formatted: None,
        }
    }
}

fn edited(message: &Message) -> bool {
    message
        .edited_timestamp
        .as_deref()
        .is_some_and(|ts| !ts.is_empty())
}

fn media_kind(mime: &str) -> &'static str {
    if mime.starts_with("image/") {
        if mime.contains("gif") {
            "sticker"
        } else {
            "image"
        }
    } else if mime.starts_with("video/") {
        "video"
    } else if mime.starts_with("audio/") {
        "audio"
    } else {
        "file"
    }
}

/// Parse Discord's ISO-8601 timestamp (`2024-05-01T12:34:56.789000+00:00`) into Unix milliseconds.
pub fn ts_ms(value: &str) -> i64 {
    let value = value.trim();
    if value.is_empty() {
        return 0;
    }
    let (date, rest) = match value.split_once('T') {
        Some(parts) => parts,
        None => return 0,
    };
    let mut date_parts = date.split('-');
    let year = date_parts.next().and_then(|y| y.parse::<i64>().ok()).unwrap_or(1970);
    let month = date_parts.next().and_then(|m| m.parse::<u32>().ok()).unwrap_or(1);
    let day = date_parts.next().and_then(|d| d.parse::<u32>().ok()).unwrap_or(1);
    let mut time_parts = rest.split([':', '+', '-', 'Z']);
    let hour = time_parts.next().and_then(|h| h.parse::<i64>().ok()).unwrap_or(0);
    let minute = time_parts.next().and_then(|m| m.parse::<i64>().ok()).unwrap_or(0);
    let seconds = time_parts.next().unwrap_or("0");
    let (second, fraction) = match seconds.split_once('.') {
        Some((s, f)) => (s.parse::<i64>().unwrap_or(0), f),
        None => (seconds.parse::<i64>().unwrap_or(0), ""),
    };
    let millis = fraction
        .chars()
        .take(3)
        .collect::<String>()
        .parse::<i64>()
        .map(|value| match fraction.len() {
            1 => value * 100,
            2 => value * 10,
            _ => value,
        })
        .unwrap_or(0);
    let days = days_from_civil(year, month, day);
    ((days * 86_400 + hour * 3_600 + minute * 60 + second) * 1_000) + millis
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = i64::from(month);
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_discord_timestamps() {
        assert_eq!(ts_ms("1970-01-01T00:00:00+00:00"), 0);
        assert_eq!(ts_ms("2024-05-01T12:34:56.789000+00:00"), 1_714_566_896_789);
        assert_eq!(ts_ms("2024-05-01T12:34:56Z"), 1_714_566_896_000);
        assert_eq!(ts_ms("garbage"), 0);
    }

    #[test]
    fn maps_channel_kinds() {
        let dm = Channel {
            id: "1".into(),
            kind: 1,
            recipients: vec![User {
                id: "9".into(),
                username: "ada".into(),
                global_name: Some("Ada".into()),
                avatar: None,
                bot: false,
            }],
            ..Default::default()
        };
        let meta = chat_meta(&dm, "").unwrap();
        assert!(meta.is_dm);
        assert_eq!(meta.name, "Ada");

        let voice = Channel {
            id: "2".into(),
            kind: 2,
            ..Default::default()
        };
        assert!(chat_meta(&voice, "").is_none());

        let guild = Guild {
            id: "5".into(),
            name: "Space".into(),
            ..Default::default()
        };
        assert!(guild_meta(&guild).is_space);
    }

    #[test]
    fn reaction_keys_cover_unicode_and_custom_emoji() {
        let unicode = Emoji {
            id: None,
            name: Some("👍".into()),
        };
        assert_eq!(emoji_key(&unicode), "👍");
        let custom = Emoji {
            id: Some("123".into()),
            name: Some("wave".into()),
        };
        assert_eq!(emoji_key(&custom), "wave:123");
    }
}