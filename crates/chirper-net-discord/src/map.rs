//! Discord payloads and their mapping onto the bridge's domain types.

use std::sync::Arc;

use serde::{Deserialize, Deserializer};
use serde_json::Value;

use chirper_bridge::model::{
    ChatMeta, Content, MediaInfo, PortalRef, Reaction, RemoteMessage, RemoteUser,
};

use crate::api::{Api, CDN};

/// Largest attachment that is downloaded into the local media store.
pub const MAX_ATTACHMENT: usize = 24 << 20;
/// Longest text of a notice built from an embed-only message, in characters.
pub const MAX_NOTICE: usize = 4000;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct User {
    pub id: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub global_name: Option<String>,
    #[serde(default)]
    pub avatar: Option<String>,
    /// Whether the account is a bot application.
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

/// A channel named in a message's text (`<#id>`).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct MentionChannel {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
}

/// A sticker attached to a message. `format_type`: 1 PNG, 2 APNG, 3 Lottie, 4 GIF.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct StickerItem {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub format_type: i64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct EmbedAuthor {
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct EmbedField {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct EmbedFooter {
    #[serde(default)]
    pub text: Option<String>,
}

/// A rich embed of a bot or webhook message. Every field is optional on the wire.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Embed {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub author: Option<EmbedAuthor>,
    #[serde(default)]
    pub fields: Vec<EmbedField>,
    #[serde(default)]
    pub footer: Option<EmbedFooter>,
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
    pub sticker_items: Vec<StickerItem>,
    #[serde(default)]
    pub embeds: Vec<Embed>,
    #[serde(default)]
    pub mentions: Vec<User>,
    #[serde(default)]
    pub mention_channels: Vec<MentionChannel>,
    #[serde(rename = "type", default)]
    pub kind: i64,
}

/// A MESSAGE_UPDATE payload. Discord sends partial objects (an embed unfurl has no content and no
/// author), so every field is optional and a partial payload always parses.
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize, Default)]
pub struct MessageUpdate {
    pub id: Option<String>,
    pub channel_id: Option<String>,
    pub guild_id: Option<String>,
    pub author: Option<User>,
    pub content: Option<String>,
    pub timestamp: Option<String>,
    pub edited_timestamp: Option<String>,
    pub message_reference: Option<MessageReference>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub reactions: Vec<ReactionCount>,
    #[serde(default)]
    pub sticker_items: Vec<StickerItem>,
    #[serde(default)]
    pub embeds: Vec<Embed>,
    #[serde(default)]
    pub mentions: Vec<User>,
    #[serde(default)]
    pub mention_channels: Vec<MentionChannel>,
    #[serde(rename = "type")]
    pub kind: Option<i64>,
}

impl MessageUpdate {
    /// Whether the update is a real edit: it carries new text and an edit timestamp.
    pub fn is_edit(&self) -> bool {
        self.content.is_some()
            && self
                .edited_timestamp
                .as_deref()
                .is_some_and(|ts| !ts.is_empty())
    }

    /// The edited message, when the payload has the author and identifiers to build one.
    pub fn as_message(&self) -> Option<Message> {
        Some(Message {
            id: self.id.clone()?,
            channel_id: self.channel_id.clone()?,
            guild_id: self.guild_id.clone(),
            author: self.author.clone()?,
            content: self.content.clone()?,
            timestamp: self.timestamp.clone().unwrap_or_default(),
            edited_timestamp: self.edited_timestamp.clone(),
            message_reference: self.message_reference.clone(),
            attachments: self.attachments.clone(),
            reactions: self.reactions.clone(),
            sticker_items: self.sticker_items.clone(),
            embeds: self.embeds.clone(),
            mentions: self.mentions.clone(),
            mention_channels: self.mention_channels.clone(),
            kind: self.kind.unwrap_or(0),
        })
    }
}

/// A reaction event (add, remove, remove emoji, remove all) without its reactor summary.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ReactionEvent {
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub channel_id: String,
    #[serde(default)]
    pub message_id: String,
    #[serde(default)]
    pub emoji: Emoji,
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
    /// User accounts receive recipients as ids; [`resolve_recipients`] turns them into users.
    #[serde(default)]
    pub recipient_ids: Vec<String>,
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
    /// The channels of the guild. `None` when the guild object has no `channels` key at all.
    #[serde(default, deserialize_with = "lenient_channels_opt")]
    pub channels: Option<Vec<Channel>>,
    /// Active threads of the guild.
    #[serde(default, deserialize_with = "lenient_channels")]
    pub threads: Vec<Channel>,
}

/// Channels are parsed one by one, so one malformed channel does not drop its whole guild.
fn parse_channels(items: Vec<Value>) -> Vec<Channel> {
    items
        .into_iter()
        .filter_map(|item| serde_json::from_value(item).ok())
        .collect()
}

fn lenient_channels<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Channel>, D::Error> {
    Ok(parse_channels(Vec::<Value>::deserialize(deserializer)?))
}

fn lenient_channels_opt<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<Channel>>, D::Error> {
    Ok(Option::<Vec<Value>>::deserialize(deserializer)?.map(parse_channels))
}

/// Discord snowflakes are decimal numbers of at most 20 digits.
pub fn is_snowflake(text: &str) -> bool {
    !text.is_empty() && text.len() <= 20 && text.bytes().all(|b| b.is_ascii_digit())
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

/// Fill a private channel's recipients from its `recipient_ids` when the payload has no
/// `recipients` array (user accounts). Ids missing from `users` stay as bare users.
pub fn resolve_recipients(channel: &mut Channel, users: &[User]) {
    if !channel.recipients.is_empty() {
        return;
    }
    channel.recipients = channel
        .recipient_ids
        .iter()
        .map(|id| {
            users
                .iter()
                .find(|user| &user.id == id)
                .cloned()
                .unwrap_or_else(|| User {
                    id: id.clone(),
                    ..Default::default()
                })
        })
        .collect();
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

/// The key the bridge uses for an emoji: the character itself, or `name:id` for a custom emoji.
pub fn emoji_key(emoji: &Emoji) -> String {
    match (&emoji.id, &emoji.name) {
        (Some(id), Some(name)) if !name.is_empty() => format!("{name}:{id}"),
        (Some(id), _) => id.clone(),
        (None, Some(name)) => name.clone(),
        (None, None) => "?".to_owned(),
    }
}

/// Convert one message, downloading its first attachment or sticker when there is one. `me_id` is
/// the logged-in user's id: their messages from other devices are marked `from_me`.
pub async fn to_remote_message(
    api: &Api,
    message: &Message,
    live: bool,
    me_id: &str,
) -> RemoteMessage {
    let text = render_markup(
        &message.content,
        &message.mentions,
        &message.mention_channels,
    );
    let content = if let Some(attachment) = message.attachments.first() {
        attachment_content(api, message, attachment, &text).await
    } else if let Some(sticker) = message.sticker_items.first() {
        sticker_content(api, sticker, &text).await
    } else if message.content.trim().is_empty() && !message.embeds.is_empty() {
        let body = embed_notice(message);
        if body.trim().is_empty() {
            unsupported()
        } else {
            Content::Notice { body }
        }
    } else {
        text_content(&text)
    };

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
        from_me: !me_id.is_empty() && message.author.id == me_id,
    }
}

async fn attachment_content(
    api: &Api,
    message: &Message,
    attachment: &Attachment,
    text: &str,
) -> Content {
    let mime = attachment
        .content_type
        .clone()
        .unwrap_or_else(|| "application/octet-stream".to_owned());
    let kind = media_kind(&mime);
    let oversized = attachment.size > MAX_ATTACHMENT as i64;
    let downloaded = if oversized {
        Err(chirper_bridge::Error::Protocol(
            "attachment is too large".to_owned(),
        ))
    } else {
        api.download(&attachment.url, MAX_ATTACHMENT).await
    };
    match downloaded {
        Ok(data) if !data.is_empty() => Content::Media {
            kind: kind.to_owned(),
            caption: if message.content.is_empty() {
                attachment.description.clone().unwrap_or_default()
            } else {
                text.to_owned()
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
            data: Arc::new(data),
            thumbnail: None,
        },
        _ => Content::Text {
            body: format!("📎 {}", attachment.filename),
            formatted: None,
        },
    }
}

/// A sticker: PNG, APNG and GIF stickers are downloaded from the sticker CDN; Lottie stickers and
/// anything unknown are described in text.
async fn sticker_content(api: &Api, sticker: &StickerItem, text: &str) -> Content {
    let name = if sticker.name.is_empty() {
        "sticker"
    } else {
        sticker.name.as_str()
    };
    let (extension, mime) = match sticker.format_type {
        1 | 2 => ("png", "image/png"),
        4 => ("gif", "image/gif"),
        _ => return sticker_text(name),
    };
    if !is_snowflake(&sticker.id) {
        return sticker_text(name);
    }
    let url = format!(
        "https://media.discordapp.net/stickers/{}.{extension}?size=320",
        sticker.id
    );
    match api.download(&url, MAX_ATTACHMENT).await {
        Ok(data) if !data.is_empty() => Content::Media {
            kind: "sticker".to_owned(),
            caption: text.to_owned(),
            info: MediaInfo {
                mime: mime.to_owned(),
                file_name: format!("{}.{extension}", sticker.id),
                size: data.len(),
                ..Default::default()
            },
            data: Arc::new(data),
            thumbnail: None,
        },
        _ => sticker_text(name),
    }
}

fn sticker_text(name: &str) -> Content {
    Content::Text {
        body: format!("[Sticker: {name}]"),
        formatted: None,
    }
}

/// The text of an embed-only message: per embed, its author, bold title, description, fields,
/// footer and link, newline separated and capped at [`MAX_NOTICE`] characters.
fn embed_notice(message: &Message) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut push = |line: &str| {
        if !line.trim().is_empty() {
            lines.push(line.to_owned());
        }
    };
    for embed in &message.embeds {
        if let Some(name) = embed.author.as_ref().and_then(|a| a.name.as_deref()) {
            push(name);
        }
        if let Some(title) = embed.title.as_deref() {
            push(&format!("**{title}**"));
        }
        if let Some(description) = embed.description.as_deref() {
            push(&render_markup(
                description,
                &message.mentions,
                &message.mention_channels,
            ));
        }
        for field in &embed.fields {
            let name = field.name.as_deref().unwrap_or_default();
            let value = field.value.as_deref().unwrap_or_default();
            if name.is_empty() {
                push(value);
            } else {
                push(&format!("{name}: {value}"));
            }
        }
        if let Some(footer) = embed.footer.as_ref().and_then(|f| f.text.as_deref()) {
            push(footer);
        }
        if let Some(url) = embed.url.as_deref() {
            push(url);
        }
    }
    lines.join("\n").chars().take(MAX_NOTICE).collect()
}

fn unsupported() -> Content {
    Content::Unsupported {
        body: "Unsupported Discord message".to_owned(),
    }
}

/// Pins, joins, boosts and calls are not conversation and never become messages.
pub fn is_system(message: &Message) -> bool {
    matches!(message.kind, 6..=12)
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

/// Render Discord's inline markup as plain text: user, role and channel mentions, custom emoji and
/// timestamps. Anything malformed is left as it is.
pub fn render_markup(text: &str, users: &[User], channels: &[MentionChannel]) -> String {
    if !text.contains('<') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let token = tail
            .find('>')
            .and_then(|end| render_token(&tail[1..end], users, channels).map(|s| (s, end + 1)));
        match token {
            Some((rendered, used)) => {
                out.push_str(&rendered);
                rest = &tail[used..];
            }
            None => {
                out.push('<');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Render the inside of one `<…>` token, or `None` when it is not a well-formed token.
fn render_token(inner: &str, users: &[User], channels: &[MentionChannel]) -> Option<String> {
    if let Some(rest) = inner.strip_prefix('@') {
        if let Some(role) = rest.strip_prefix('&') {
            return is_snowflake(role).then(|| "@role".to_owned());
        }
        let id = rest.strip_prefix('!').unwrap_or(rest);
        if !is_snowflake(id) {
            return None;
        }
        return Some(match users.iter().find(|user| user.id == id) {
            Some(user) => format!("@{}", display_name(user)),
            None => "@unknown-user".to_owned(),
        });
    }
    if let Some(id) = inner.strip_prefix('#') {
        if !is_snowflake(id) {
            return None;
        }
        let name = channels
            .iter()
            .find(|channel| channel.id == id)
            .map(|channel| channel.name.as_str())
            .filter(|name| !name.is_empty());
        return Some(format!("#{}", name.unwrap_or("channel")));
    }
    if let Some(time) = inner.strip_prefix("t:") {
        return render_time(time);
    }
    // Custom emoji: `:name:id`, or `a:name:id` when animated.
    let body = inner.strip_prefix('a').unwrap_or(inner).strip_prefix(':')?;
    let mut parts = body.split(':');
    let name = parts.next()?;
    let id = parts.next()?;
    if parts.next().is_some()
        || name.is_empty()
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        || !is_snowflake(id)
    {
        return None;
    }
    Some(format!(":{name}:"))
}

/// `UNIX` or `UNIX:style` of a `<t:…>` token, as an RFC 3339 UTC time.
fn render_time(time: &str) -> Option<String> {
    let mut parts = time.split(':');
    let seconds = parts.next()?;
    if let Some(style) = parts.next()
        && (!matches!(style, "t" | "T" | "d" | "D" | "f" | "F" | "R") || parts.next().is_some())
    {
        return None;
    }
    if seconds.is_empty() || seconds.len() > 15 || !seconds.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(format_rfc3339(seconds.parse().ok()?))
}

/// Unix seconds as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn format_rfc3339(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let within = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        within / 3_600,
        within % 3_600 / 60,
        within % 60
    )
}

/// Year, month and day of a day count since 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month as u32, day as u32)
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
    let year = date_parts
        .next()
        .and_then(|y| y.parse::<i64>().ok())
        .unwrap_or(1970);
    let month = date_parts
        .next()
        .and_then(|m| m.parse::<u32>().ok())
        .unwrap_or(1);
    let day = date_parts
        .next()
        .and_then(|d| d.parse::<u32>().ok())
        .unwrap_or(1);
    let mut time_parts = rest.split([':', '+', '-', 'Z']);
    let hour = time_parts
        .next()
        .and_then(|h| h.parse::<i64>().ok())
        .unwrap_or(0);
    let minute = time_parts
        .next()
        .and_then(|m| m.parse::<i64>().ok())
        .unwrap_or(0);
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

    fn user(id: &str, name: &str) -> User {
        User {
            id: id.into(),
            username: name.into(),
            global_name: None,
            avatar: None,
            bot: false,
        }
    }

    fn channel(id: &str, name: &str) -> MentionChannel {
        MentionChannel {
            id: id.into(),
            name: name.into(),
        }
    }

    #[test]
    fn parses_discord_timestamps() {
        assert_eq!(ts_ms("1970-01-01T00:00:00+00:00"), 0);
        assert_eq!(ts_ms("2024-05-01T12:34:56.789000+00:00"), 1_714_566_896_789);
        assert_eq!(ts_ms("2024-05-01T12:34:56Z"), 1_714_566_896_000);
        assert_eq!(ts_ms("garbage"), 0);
    }

    #[test]
    fn formats_unix_seconds_as_rfc3339() {
        assert_eq!(format_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_rfc3339(1_714_566_896), "2024-05-01T12:34:56Z");
        assert_eq!(format_rfc3339(951_782_400), "2000-02-29T00:00:00Z");
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
        let meta = chat_meta(&dm, "").expect("a DM is a chat");
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

    #[test]
    fn dm_recipients_resolve_from_recipient_ids() {
        let payload = serde_json::json!({
            "id": "77", "type": 1, "recipient_ids": ["9", "10"]
        });
        let mut dm: Channel = serde_json::from_value(payload).expect("channel parses");
        let users = vec![user("9", "ada")];
        resolve_recipients(&mut dm, &users);
        assert_eq!(dm.recipients.len(), 2);
        assert_eq!(dm.recipients[0].username, "ada");
        assert_eq!(dm.recipients[1].id, "10");
        let meta = chat_meta(&dm, "").expect("a DM is a chat");
        assert_eq!(meta.dm_user.map(|u| u.id), Some("9".to_owned()));
    }

    #[test]
    fn existing_recipients_are_not_replaced() {
        let mut dm = Channel {
            id: "77".into(),
            kind: 1,
            recipients: vec![user("3", "bob")],
            recipient_ids: vec!["9".into()],
            ..Default::default()
        };
        resolve_recipients(&mut dm, &[user("9", "ada")]);
        assert_eq!(dm.recipients.len(), 1);
        assert_eq!(dm.recipients[0].id, "3");
    }

    #[test]
    fn guild_channels_survive_one_malformed_entry() {
        let guild: Guild = serde_json::from_value(serde_json::json!({
            "id": "5",
            "name": "Space",
            "channels": [{"id": "1", "type": 0}, {"name": "no id"}],
            "threads": [{"id": "2", "type": 11}]
        }))
        .expect("guild parses");
        assert_eq!(guild.channels.map(|c| c.len()), Some(1));
        assert_eq!(guild.threads.len(), 1);

        let bare: Guild =
            serde_json::from_value(serde_json::json!({"id": "6", "name": "x"})).expect("parses");
        assert!(bare.channels.is_none(), "a missing key means fall back");
    }

    #[test]
    fn partial_message_updates_parse_and_only_edits_emit() {
        let unfurl: MessageUpdate = serde_json::from_value(serde_json::json!({
            "id": "1", "channel_id": "2", "embeds": [{"title": "Docs"}]
        }))
        .expect("a partial update parses");
        assert!(!unfurl.is_edit());

        let edit: MessageUpdate = serde_json::from_value(serde_json::json!({
            "id": "1", "channel_id": "2", "content": "fixed",
            "edited_timestamp": "2024-05-01T12:00:00+00:00",
            "author": {"id": "3", "username": "ada"}
        }))
        .expect("an edit parses");
        assert!(edit.is_edit());
        let message = edit.as_message().expect("the edit has an author");
        assert_eq!(message.content, "fixed");
        assert_eq!(message.author.id, "3");

        let no_timestamp: MessageUpdate = serde_json::from_value(serde_json::json!({
            "id": "1", "channel_id": "2", "content": "x", "edited_timestamp": null
        }))
        .expect("parses");
        assert!(!no_timestamp.is_edit());
    }

    #[test]
    fn mentions_render_from_the_message_arrays() {
        let users = [user("42", "ada")];
        let channels = [channel("7", "general")];
        assert_eq!(
            render_markup("hi <@42> and <@!42>", &users, &channels),
            "hi @ada and @ada"
        );
        assert_eq!(render_markup("<@99>", &users, &channels), "@unknown-user");
        assert_eq!(render_markup("<@&555>", &users, &channels), "@role");
        assert_eq!(render_markup("see <#7>", &users, &channels), "see #general");
        assert_eq!(render_markup("see <#8>", &users, &channels), "see #channel");
    }

    #[test]
    fn emoji_render_as_their_names() {
        assert_eq!(render_markup("<:wave:123>", &[], &[]), ":wave:");
        assert_eq!(render_markup("<a:dance:456>", &[], &[]), ":dance:");
        assert_eq!(render_markup("<:bad name:1>", &[], &[]), "<:bad name:1>");
    }

    #[test]
    fn timestamps_render_as_utc_times() {
        assert_eq!(
            render_markup("at <t:1714566896:R>", &[], &[]),
            "at 2024-05-01T12:34:56Z"
        );
        assert_eq!(
            render_markup("<t:1714566896>", &[], &[]),
            "2024-05-01T12:34:56Z"
        );
        assert_eq!(
            render_markup("<t:1714566896:x>", &[], &[]),
            "<t:1714566896:x>"
        );
    }

    #[test]
    fn malformed_markup_is_left_untouched() {
        assert_eq!(render_markup("<@abc", &[], &[]), "<@abc");
        assert_eq!(
            render_markup("<@abc> <@12>", &[], &[]),
            "<@abc> @unknown-user"
        );
        assert_eq!(render_markup("<<@x>", &[], &[]), "<<@x>");
        assert_eq!(render_markup("a < b > c", &[], &[]), "a < b > c");
        assert_eq!(render_markup("<#>", &[], &[]), "<#>");
        assert_eq!(render_markup("<t:>", &[], &[]), "<t:>");
    }

    #[test]
    fn embed_notices_join_the_visible_parts() {
        let message: Message = serde_json::from_value(serde_json::json!({
            "id": "1", "channel_id": "2", "author": {"id": "3", "username": "hook"},
            "content": "",
            "embeds": [{
                "title": "Build 42",
                "description": "passed",
                "url": "https://ci.example/42",
                "author": {"name": "CI"},
                "fields": [{"name": "branch", "value": "main"}, {"value": "bare"}],
                "footer": {"text": "nightly"}
            }]
        }))
        .expect("message parses");
        assert_eq!(
            embed_notice(&message),
            "CI\n**Build 42**\npassed\nbranch: main\nbare\nnightly\nhttps://ci.example/42"
        );
    }

    #[test]
    fn embed_notices_are_capped() {
        let long = "x".repeat(MAX_NOTICE + 500);
        let message: Message = serde_json::from_value(serde_json::json!({
            "id": "1", "channel_id": "2", "author": {"id": "3"},
            "embeds": [{"description": long}]
        }))
        .expect("message parses");
        assert_eq!(embed_notice(&message).chars().count(), MAX_NOTICE);
    }

    #[test]
    fn sticker_messages_parse_with_their_format() {
        let message: Message = serde_json::from_value(serde_json::json!({
            "id": "1", "channel_id": "2", "author": {"id": "3"},
            "sticker_items": [{"id": "9", "name": "wave", "format_type": 3}]
        }))
        .expect("message parses");
        assert_eq!(message.sticker_items[0].format_type, 3);
        assert_eq!(message.sticker_items[0].name, "wave");
    }
}
