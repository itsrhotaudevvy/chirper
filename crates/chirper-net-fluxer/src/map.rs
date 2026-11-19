//! Fluxer payloads and their mapping onto the bridge's domain types.
//!
//! Port of `fluxerapi/models.go`, `connector/ids.go`, `convert.go`, `chatinfo.go` and `markdown.go`.
//! Every model is partial-safe: all fields default, so a truncated gateway payload still parses.
//! Identity rules (portal, part and message ids) are the ones the Go connector used, because
//! existing databases resolve through them.

use std::sync::Arc;

use serde::Deserialize;

use chirper_bridge::model::{
    ChatMeta, Content, MediaInfo, PortalRef, Reaction, RemoteMessage, RemoteUser,
};

use crate::api::Client;

/// The message type of an ordinary message.
const TYPE_DEFAULT: i64 = 0;
/// The message type of a reply.
const TYPE_REPLY: i64 = 19;
/// A message reference of this type is a forward, not a reply.
const REFERENCE_FORWARD: i64 = 1;

/// Channel types Chirper shows (docs: http-api/channels.mdx).
const CHANNEL_GUILD_TEXT: i64 = 0;
const CHANNEL_DM: i64 = 1;
const CHANNEL_GROUP_DM: i64 = 3;
const CHANNEL_GUILD_ANNOUNCEMENT: i64 = 5;

pub const UNSUPPORTED_NOTICE: &str = "[unsupported message]";
pub const SYSTEM_NOTICE: &str = "[system message]";
pub const ATTACHMENT_NOTICE: &str = "[attachment unavailable]";

/// A user object.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct User {
    pub id: String,
    pub username: String,
    pub global_name: Option<String>,
    pub avatar: Option<String>,
}

impl User {
    /// The global name when set, otherwise the username.
    pub fn display_name(&self) -> String {
        match &self.global_name {
            Some(name) if !name.is_empty() => name.clone(),
            _ => self.username.clone(),
        }
    }
}

/// A guild object, limited to the fields Chirper uses.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Guild {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
}

/// A channel object, limited to the fields Chirper uses.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Channel {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: i64,
    pub guild_id: String,
    pub name: Option<String>,
    pub topic: Option<String>,
    pub icon: Option<String>,
    pub recipients: Vec<User>,
}

/// A message object.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Message {
    pub id: String,
    pub channel_id: String,
    pub guild_id: String,
    pub author: User,
    #[serde(rename = "type")]
    pub kind: i64,
    pub content: String,
    pub timestamp: String,
    pub attachments: Vec<Attachment>,
    pub reactions: Vec<MessageReaction>,
    pub message_reference: Option<MessageReference>,
}

/// One attachment of a message.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Attachment {
    pub id: String,
    pub filename: String,
    pub content_type: String,
    pub url: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub duration: Option<i64>,
    pub expired: bool,
}

/// A message reference: the reply or forward target.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct MessageReference {
    pub message_id: String,
    #[serde(rename = "type")]
    pub kind: i64,
}

/// One reaction of a message's summary.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct MessageReaction {
    pub emoji: Emoji,
    pub count: i64,
    pub me: bool,
}

/// An emoji. `id` is set only for custom emoji.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Emoji {
    pub id: Option<String>,
    pub name: String,
}

impl Emoji {
    /// The key a reaction is stored under: the character, or `name:id` for a custom emoji.
    pub fn key(&self) -> String {
        match &self.id {
            Some(id) if !id.is_empty() => format!("{}:{id}", self.name),
            _ => self.name.clone(),
        }
    }
}

/// The READY dispatch.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Ready {
    pub session_id: String,
    pub user: User,
    pub guilds: Vec<GuildState>,
    pub private_channels: Vec<Channel>,
}

/// A guild as READY and GUILD_CREATE describe it.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct GuildState {
    pub id: String,
    pub properties: Guild,
    pub channels: Vec<Channel>,
    pub unavailable: bool,
}

/// The payload of GUILD_DELETE.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct GuildRef {
    pub id: String,
    pub unavailable: bool,
}

/// The payload of MESSAGE_REACTION_ADD and MESSAGE_REACTION_REMOVE.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ReactionEvent {
    pub channel_id: String,
    pub message_id: String,
    pub guild_id: String,
}

/// The payload of TYPING_START.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct TypingEvent {
    pub channel_id: String,
    pub user_id: String,
    pub guild_id: String,
}

/// The payload of MESSAGE_DELETE.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct MessageDelete {
    pub id: String,
    pub channel_id: String,
    pub guild_id: String,
}

/// The payload of MESSAGE_DELETE_BULK.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct MessageDeleteBulk {
    pub ids: Vec<String>,
    pub channel_id: String,
    pub guild_id: String,
}

/// The payload of MESSAGE_ACK.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct MessageAck {
    pub channel_id: String,
    pub message_id: String,
}

/// The payload of CHANNEL_RECIPIENT_ADD and CHANNEL_RECIPIENT_REMOVE.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct RecipientEvent {
    pub channel_id: String,
    pub user: User,
}

/// The payload of GUILD_MEMBER_ADD and GUILD_MEMBER_REMOVE.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct MemberEvent {
    pub guild_id: String,
    pub user: User,
}

/// The portal of a guild channel, or of a direct chat for the login. Guild channels are shared
/// between accounts, so their receiver stays empty; a direct chat is scoped to the login.
pub fn channel_portal(channel: &str, guild: Option<&str>, login_id: &str) -> PortalRef {
    match guild.filter(|g| !g.is_empty()) {
        Some(_) => PortalRef {
            id: channel.to_owned(),
            receiver: String::new(),
        },
        None => PortalRef {
            id: channel.to_owned(),
            receiver: login_id.to_owned(),
        },
    }
}

/// The portal of a guild, which Chirper shows as a space.
pub fn guild_portal(guild: &str) -> PortalRef {
    PortalRef {
        id: guild.to_owned(),
        receiver: String::new(),
    }
}

/// The part id of an attachment. The text part of a message is the empty part id.
pub fn part_id(attachment: &str) -> String {
    attachment.to_owned()
}

/// Whether a channel type is one that holds messages Chirper shows.
pub fn is_message_channel(kind: i64) -> bool {
    matches!(
        kind,
        CHANNEL_GUILD_TEXT | CHANNEL_GUILD_ANNOUNCEMENT | CHANNEL_DM | CHANNEL_GROUP_DM
    )
}

/// An image or icon URL under the media base. Animated hashes start with `a_` and use gif.
pub fn image_url(media: &str, path: &str, hash: &str) -> String {
    let ext = if hash.starts_with("a_") {
        "gif"
    } else {
        "webp"
    };
    format!(
        "{}/{}/{hash}.{ext}",
        media.trim_end_matches('/'),
        path.trim_matches('/')
    )
}

fn cdn(media: &str, path: &str, id: &str, hash: Option<&str>) -> String {
    match hash {
        Some(hash) if !hash.is_empty() => image_url(media, &format!("{path}/{id}"), hash),
        _ => String::new(),
    }
}

/// A user as a bridge user, with the avatar URL under the media base.
pub fn remote_user(user: &User, media: &str) -> RemoteUser {
    RemoteUser {
        id: user.id.clone(),
        display_name: user.display_name(),
        avatar_url: cdn(media, "avatars", &user.id, user.avatar.as_deref()),
        username: user.username.clone(),
        phone: String::new(),
    }
}

/// The parent of a guild channel: the guild, its name and its icon hash.
pub struct Parent<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub icon: Option<&'a str>,
}

/// A channel as a chat. A guild channel is a child of its guild; a DM or group DM belongs to the
/// login. Other channel types are not chats.
pub fn channel_meta(
    channel: &Channel,
    parent: Option<Parent<'_>>,
    login_id: &str,
    media: &str,
) -> Option<ChatMeta> {
    if !is_message_channel(channel.kind) {
        return None;
    }
    if let Some(parent) = parent {
        return Some(ChatMeta {
            portal: channel_portal(&channel.id, Some(parent.id), login_id),
            name: channel.name.clone().unwrap_or_default(),
            topic: channel.topic.clone().unwrap_or_default(),
            avatar_url: cdn(media, "icons", parent.id, parent.icon),
            is_dm: false,
            is_space: false,
            member_count: 0,
            parent: Some(guild_portal(parent.id)),
            parent_name: parent.name.to_owned(),
            dm_user: None,
        });
    }
    let portal = channel_portal(&channel.id, None, login_id);
    let named = channel.name.as_deref().filter(|n| !n.is_empty());
    if channel.kind == CHANNEL_GROUP_DM {
        let name = match named {
            Some(name) => name.to_owned(),
            None => channel
                .recipients
                .iter()
                .map(User::display_name)
                .collect::<Vec<_>>()
                .join(", "),
        };
        let avatar_url = match channel.icon.as_deref().filter(|h| !h.is_empty()) {
            Some(icon) => cdn(media, "channel-icons", &channel.id, Some(icon)),
            None => channel
                .recipients
                .first()
                .map(|u| cdn(media, "avatars", &u.id, u.avatar.as_deref()))
                .unwrap_or_default(),
        };
        return Some(ChatMeta {
            portal,
            name,
            topic: String::new(),
            avatar_url,
            is_dm: false,
            is_space: false,
            member_count: channel.recipients.len() as i64 + 1,
            parent: None,
            parent_name: String::new(),
            dm_user: None,
        });
    }
    let partner = channel.recipients.first();
    Some(ChatMeta {
        portal,
        name: named
            .map(str::to_owned)
            .or_else(|| partner.map(User::display_name))
            .unwrap_or_default(),
        topic: String::new(),
        avatar_url: partner
            .map(|u| cdn(media, "avatars", &u.id, u.avatar.as_deref()))
            .unwrap_or_default(),
        is_dm: true,
        is_space: false,
        member_count: 2,
        parent: None,
        parent_name: String::new(),
        dm_user: partner.map(|u| remote_user(u, media)),
    })
}

/// A guild as a space chat, the parent of its channels.
pub fn guild_meta(guild: &Guild, media: &str) -> ChatMeta {
    ChatMeta {
        portal: guild_portal(&guild.id),
        name: if guild.name.is_empty() {
            guild.id.clone()
        } else {
            guild.name.clone()
        },
        topic: String::new(),
        avatar_url: cdn(media, "icons", &guild.id, guild.icon.as_deref()),
        is_dm: false,
        is_space: true,
        member_count: 0,
        parent: None,
        parent_name: String::new(),
        dm_user: None,
    }
}

/// The text of a message as content: a notice for a system message, `None` when there is no text.
pub fn text_content(message: &Message) -> Option<Content> {
    if message.kind != TYPE_DEFAULT && message.kind != TYPE_REPLY {
        return Some(Content::Notice {
            body: SYSTEM_NOTICE.to_owned(),
        });
    }
    if message.content.is_empty() {
        return None;
    }
    Some(Content::Text {
        body: message.content.clone(),
        formatted: None,
    })
}

/// The reply target of a message, if it is a reply rather than a forward.
fn reply_target(message: &Message) -> Option<String> {
    message
        .message_reference
        .as_ref()
        .filter(|r| !r.message_id.is_empty() && r.kind != REFERENCE_FORWARD)
        .map(|r| r.message_id.clone())
}

/// The media type of an attachment, as the bridge names it.
pub fn media_kind(mime: &str) -> &'static str {
    if mime.starts_with("image/") {
        "image"
    } else if mime.starts_with("video/") {
        "video"
    } else if mime.starts_with("audio/") {
        "audio"
    } else {
        "file"
    }
}

/// What the parts of one message share.
struct Frame<'a> {
    message: &'a Message,
    portal: &'a PortalRef,
    sender: RemoteUser,
    from_me: bool,
    reply_to: Option<String>,
    live: bool,
}

impl Frame<'_> {
    fn make(&self, part: &str, content: Content, edit_of: Option<String>) -> RemoteMessage {
        RemoteMessage {
            portal: self.portal.clone(),
            id: self.message.id.clone(),
            part: part.to_owned(),
            sender: self.sender.clone(),
            timestamp_ms: ts_ms(&self.message.timestamp),
            content,
            reply_to: self.reply_to.clone(),
            edit_of,
            live: self.live,
            from_me: self.from_me,
        }
    }
}

impl<'a> Frame<'a> {
    fn new(
        message: &'a Message,
        portal: &'a PortalRef,
        me_id: &str,
        media: &str,
        live: bool,
    ) -> Self {
        Self {
            message,
            portal,
            sender: remote_user(&message.author, media),
            from_me: !me_id.is_empty() && message.author.id == me_id,
            reply_to: reply_target(message),
            live,
        }
    }
}

/// Convert a message into one bridge message per part: the text first, then one per attachment,
/// all sharing the message id. A message with neither becomes an unsupported notice.
pub async fn messages_from(
    client: &Client,
    message: &Message,
    portal: &PortalRef,
    me_id: &str,
    live: bool,
) -> Vec<RemoteMessage> {
    let media = client.media_base();
    let frame = Frame::new(message, portal, me_id, media, live);
    let mut out = Vec::new();
    if let Some(content) = text_content(message) {
        out.push(frame.make("", content, None));
    }
    for attachment in &message.attachments {
        let content = attachment_content(client, attachment).await;
        out.push(frame.make(&part_id(&attachment.id), content, None));
    }
    if out.is_empty() {
        out.push(frame.make(
            "",
            Content::Unsupported {
                body: UNSUPPORTED_NOTICE.to_owned(),
            },
            None,
        ));
    }
    out
}

/// The edit of a message's text. Attachments are not edited, and a partial update without an
/// author or without text changes nothing.
pub fn edit_from(
    message: &Message,
    portal: &PortalRef,
    me_id: &str,
    media: &str,
) -> Option<RemoteMessage> {
    if message.id.is_empty() || message.author.id.is_empty() {
        return None;
    }
    let content = text_content(message)?;
    let frame = Frame::new(message, portal, me_id, media, true);
    Some(frame.make("", content, Some(message.id.clone())))
}

/// Download an attachment into a media content. A missing, expired or unreachable attachment
/// becomes a notice, so the rest of the message still shows.
async fn attachment_content(client: &Client, attachment: &Attachment) -> Content {
    let url = match attachment.url.as_deref() {
        Some(url) if !attachment.expired => url,
        _ => return notice(ATTACHMENT_NOTICE),
    };
    match client.download(url).await {
        Ok(data) => Content::Media {
            kind: media_kind(&attachment.content_type).to_owned(),
            caption: String::new(),
            info: MediaInfo {
                mime: attachment.content_type.clone(),
                file_name: attachment.filename.clone(),
                size: data.len(),
                width: attachment.width.unwrap_or(0),
                height: attachment.height.unwrap_or(0),
                duration_ms: attachment.duration.unwrap_or(0).saturating_mul(1000),
                blurhash: String::new(),
            },
            data: Arc::new(data),
            thumbnail: None,
        },
        Err(err) => {
            log::warn!("fluxer: attachment could not be bridged: {err}");
            notice(ATTACHMENT_NOTICE)
        }
    }
}

fn notice(body: &str) -> Content {
    Content::Notice {
        body: body.to_owned(),
    }
}

/// The reaction summary of a message, as the account sees it.
pub fn reactions(message: &Message) -> Vec<Reaction> {
    message
        .reactions
        .iter()
        .map(|reaction| Reaction {
            key: reaction.emoji.key(),
            count: reaction.count,
            from_me: reaction.me,
            senders: Vec::new(),
        })
        .collect()
}

/// Parse an RFC 3339 timestamp into Unix milliseconds. A malformed value gives 0, which means
/// "unknown" to the bridge.
pub fn ts_ms(value: &str) -> i64 {
    parse_rfc3339(value.trim()).unwrap_or(0)
}

fn parse_rfc3339(value: &str) -> Option<i64> {
    let (date, rest) = value.split_once(['T', 't', ' '])?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let split_at = rest.find(['Z', 'z', '+', '-']).unwrap_or(rest.len());
    let (clock, offset) = rest.split_at(split_at);
    let (hms, fraction) = match clock.split_once('.') {
        Some((hms, fraction)) => (hms, fraction),
        None => (clock, ""),
    };
    let mut clock_parts = hms.split(':');
    let hour: i64 = clock_parts.next()?.parse().ok()?;
    let minute: i64 = clock_parts.next()?.parse().ok()?;
    let second: i64 = clock_parts.next()?.parse().ok()?;
    if clock_parts.next().is_some() || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let millis = fraction_millis(fraction)?;
    let offset_seconds = match offset {
        "" | "Z" | "z" => 0,
        other => {
            let sign = match other.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let body = other.get(1..)?;
            let (hh, mm) = body.split_once(':')?;
            let hh: i64 = hh.parse().ok()?;
            let mm: i64 = mm.parse().ok()?;
            sign * (hh * 3_600 + mm * 60)
        }
    };
    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_seconds;
    Some(seconds * 1_000 + millis)
}

/// The first three digits of a fraction of a second, as milliseconds.
fn fraction_millis(fraction: &str) -> Option<i64> {
    if fraction.is_empty() {
        return Some(0);
    }
    if !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let digits: String = fraction.chars().take(3).collect();
    let value: i64 = digits.parse().ok()?;
    Some(match digits.len() {
        1 => value * 100,
        2 => value * 10,
        _ => value,
    })
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

/// Port of `markdown.go`: the HTML subset Matrix clients send, rewritten as Fluxer Markdown.
///
/// The Rust core sends plain text today, so nothing calls this yet; it is kept so that the
/// Go connector's conversion rules stay tested and ready for a formatted send path.
#[allow(dead_code)]
pub mod markdown {
    use std::sync::LazyLock;

    use regex::{Captures, Regex};

    static PRE_OPEN: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)<pre[^>]*>"));
    static PRE_CLOSE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)</pre>"));
    static INLINE_CODE: LazyLock<Regex> = LazyLock::new(|| re(r"(?is)<code[^>]*>(.*?)</code>"));
    static ANCHOR: LazyLock<Regex> =
        LazyLock::new(|| re(r#"(?is)<a\s[^>]*?href="([^"]*)"[^>]*>(.*?)</a>"#));
    static STRONG: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)</?(strong|b)>"));
    static EMPHASIS: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)</?(em|i)>"));
    static STRIKE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)</?(del|s|strike)>"));
    static BREAK: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)<br\s*/?>"));
    static LIST_ITEM: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)<li[^>]*>"));
    static BLOCK_END: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)</(p|div|blockquote|h[1-6])>"));
    static TAG: LazyLock<Regex> = LazyLock::new(|| re(r"(?s)<[^>]+>"));

    /// The patterns are constants, and the tests compile every one of them.
    fn re(pattern: &str) -> Regex {
        Regex::new(pattern).expect("constant pattern compiles")
    }

    /// Convert the HTML a Matrix client sends into Fluxer Markdown. Unknown tags are dropped and
    /// their text kept.
    pub fn html_to_markdown(src: &str) -> String {
        let s = PRE_OPEN.replace_all(src, "\n```\n");
        let s = PRE_CLOSE.replace_all(&s, "\n```\n");
        let s = INLINE_CODE.replace_all(&s, "`$1`");
        let s = ANCHOR.replace_all(&s, |caps: &Captures| anchor_markdown(&caps[1], &caps[2]));
        let s = STRONG.replace_all(&s, "**");
        let s = EMPHASIS.replace_all(&s, "*");
        let s = STRIKE.replace_all(&s, "~~");
        let s = BREAK.replace_all(&s, "\n");
        let s = LIST_ITEM.replace_all(&s, "\n- ");
        let s = BLOCK_END.replace_all(&s, "\n");
        let s = TAG.replace_all(&s, "");
        html_unescape(&s).trim().to_owned()
    }

    /// `<a href="u">text</a>` becomes `[text](u)`, or the bare URL when the text is the URL.
    fn anchor_markdown(raw_href: &str, raw_text: &str) -> String {
        let href = html_unescape(raw_href);
        let text = TAG.replace_all(raw_text, "").into_owned();
        if text.trim().is_empty() || text == href {
            href
        } else {
            format!("[{text}]({href})")
        }
    }

    /// Decode the named and numeric references a Matrix client actually sends.
    pub fn html_unescape(src: &str) -> String {
        let mut out = String::with_capacity(src.len());
        let mut rest = src;
        while let Some(pos) = rest.find('&') {
            out.push_str(&rest[..pos]);
            let after = &rest[pos + 1..];
            let entity = after
                .find(';')
                .filter(|semi| *semi <= 10)
                .and_then(|semi| decode_entity(&after[..semi]).map(|ch| (semi, ch)));
            if let Some((semi, ch)) = entity {
                out.push(ch);
                rest = &after[semi + 1..];
                continue;
            }
            out.push('&');
            rest = after;
        }
        out.push_str(rest);
        out
    }

    fn decode_entity(name: &str) -> Option<char> {
        match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            _ => {
                if let Some(hex) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
                    u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
                } else if let Some(dec) = name.strip_prefix('#') {
                    dec.parse::<u32>().ok().and_then(char::from_u32)
                } else {
                    None
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::markdown::{html_to_markdown, html_unescape};
    use super::*;

    fn msg(json: &str) -> Message {
        serde_json::from_str(json).expect("test payload is valid JSON")
    }

    fn client() -> Client {
        Client::new(
            reqwest::Client::new(),
            crate::discovery::Endpoints {
                api: "https://api.example.test".to_owned(),
                gateway: "wss://gw.example.test".to_owned(),
                media: "https://media.example.test".to_owned(),
            },
            crate::api::Credential::anonymous(),
        )
    }

    fn text_of(content: &Content) -> Option<&str> {
        match content {
            Content::Text { body, .. } => Some(body),
            _ => None,
        }
    }

    // ---- convert_test.go ----

    #[test]
    fn text_part_for_user_messages() {
        let m = Message {
            kind: TYPE_DEFAULT,
            content: "hello".to_owned(),
            ..Default::default()
        };
        assert_eq!(text_of(&text_content(&m).expect("text")), Some("hello"));
        let reply_empty = Message {
            kind: TYPE_REPLY,
            content: String::new(),
            ..Default::default()
        };
        assert!(
            text_content(&reply_empty).is_none(),
            "empty content has no text part"
        );
    }

    #[test]
    fn system_message_is_a_notice() {
        let m = Message {
            kind: 7,
            content: String::new(),
            ..Default::default()
        };
        match text_content(&m) {
            Some(Content::Notice { body }) => assert_eq!(body, SYSTEM_NOTICE),
            other => panic!("expected a notice, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn reply_points_at_its_target_and_forward_does_not() {
        let portal = channel_portal("2", Some("9"), "100");
        let reply = msg(r#"{"id":"2","channel_id":"2","type":19,"content":"answer",
                "message_reference":{"message_id":"1","type":0}}"#);
        let parts = messages_from(&client(), &reply, &portal, "100", true).await;
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].reply_to.as_deref(), Some("1"));

        let forward = msg(r#"{"id":"3","channel_id":"2","content":"forwarded",
                "message_reference":{"message_id":"1","type":1}}"#);
        let parts = messages_from(&client(), &forward, &portal, "100", true).await;
        assert!(parts[0].reply_to.is_none(), "a forward is not a reply");
    }

    #[tokio::test]
    async fn message_without_content_or_attachments_is_unsupported() {
        let portal = channel_portal("2", Some("9"), "100");
        let m = msg(r#"{"id":"4","channel_id":"2","type":0}"#);
        let parts = messages_from(&client(), &m, &portal, "100", true).await;
        assert_eq!(parts.len(), 1);
        match &parts[0].content {
            Content::Unsupported { body } => assert_eq!(body, UNSUPPORTED_NOTICE),
            other => panic!("expected unsupported, got {other:?}"),
        }
    }

    #[test]
    fn edit_only_touches_the_text_part() {
        let portal = channel_portal("2", Some("9"), "100");
        let m = msg(r#"{"id":"9","channel_id":"2","type":0,"content":"fixed",
                "author":{"id":"200","username":"ada"}}"#);
        let edit = edit_from(&m, &portal, "100", "https://media.example.test").expect("edit");
        assert_eq!(edit.part, "");
        assert_eq!(edit.edit_of.as_deref(), Some("9"));
        assert_eq!(text_of(&edit.content), Some("fixed"));
        assert!(!edit.from_me);
    }

    #[test]
    fn partial_edit_without_author_or_text_changes_nothing() {
        let portal = channel_portal("2", Some("9"), "100");
        let no_author = msg(r#"{"id":"9","channel_id":"2","content":"fixed"}"#);
        assert!(edit_from(&no_author, &portal, "100", "").is_none());
        let no_text = msg(r#"{"id":"9","channel_id":"2","content":"","author":{"id":"200"}}"#);
        assert!(edit_from(&no_text, &portal, "100", "").is_none());
    }

    #[test]
    fn emoji_keys() {
        let custom = Emoji {
            id: Some("123".to_owned()),
            name: "party".to_owned(),
        };
        assert_eq!(custom.key(), "party:123");
        let unicode = Emoji {
            id: None,
            name: "👍".to_owned(),
        };
        assert_eq!(unicode.key(), "👍");
    }

    #[test]
    fn reaction_summary_keeps_counts_and_own_flag() {
        let m = msg(r#"{"id":"1","channel_id":"2","reactions":[
                {"emoji":{"name":"👍"},"count":3,"me":true},
                {"emoji":{"id":"77","name":"wave"},"count":1}]}"#);
        let got = reactions(&m);
        assert_eq!(got.len(), 2);
        assert_eq!(
            (got[0].key.as_str(), got[0].count, got[0].from_me),
            ("👍", 3, true)
        );
        assert_eq!(got[1].key, "wave:77");
        assert!(!got[1].from_me);
    }

    // ---- ids_test.go and the portal/part rules of the spec ----

    #[test]
    fn portal_key_is_scoped_for_direct_chats_only() {
        let dm = channel_portal("555", None, "100");
        assert_eq!((dm.id.as_str(), dm.receiver.as_str()), ("555", "100"));
        let guild = channel_portal("777", Some("9"), "100");
        assert_eq!((guild.id.as_str(), guild.receiver.as_str()), ("777", ""));
        let space = guild_portal("9");
        assert_eq!((space.id.as_str(), space.receiver.as_str()), ("9", ""));
    }

    #[test]
    fn own_messages_are_flagged_from_me() {
        let portal = channel_portal("2", None, "100");
        let mine = msg(r#"{"id":"1","channel_id":"2","content":"x","author":{"id":"100"}}"#);
        let parts = text_remote(&mine, &portal, "100");
        assert!(parts.from_me);
        let theirs = msg(r#"{"id":"1","channel_id":"2","content":"x","author":{"id":"200"}}"#);
        assert!(!text_remote(&theirs, &portal, "100").from_me);
    }

    /// The first part of a text-only message, built without a client (no download is needed).
    fn text_remote(m: &Message, portal: &PortalRef, me: &str) -> RemoteMessage {
        let content = text_content(m).unwrap_or(Content::Notice {
            body: String::new(),
        });
        Frame::new(m, portal, me, "", true).make("", content, None)
    }

    #[test]
    fn part_ids_are_empty_for_text_and_the_attachment_id_otherwise() {
        assert_eq!(part_id("att-1"), "att-1");
        let text = text_remote(
            &msg(r#"{"id":"1","channel_id":"2","content":"hi"}"#),
            &channel_portal("2", None, "100"),
            "100",
        );
        assert_eq!(text.part, "");
    }

    #[test]
    fn mime_maps_to_media_kind() {
        let cases = [
            ("image/png", "image"),
            ("video/mp4", "video"),
            ("audio/ogg", "audio"),
            ("application/pdf", "file"),
            ("", "file"),
        ];
        for (mime, want) in cases {
            assert_eq!(media_kind(mime), want, "mime {mime:?}");
        }
    }

    // ---- markdown_test.go ----

    #[test]
    fn html_to_markdown_cases() {
        let cases = [
            ("plain text", "plain text"),
            ("<strong>bold</strong> and <em>it</em>", "**bold** and *it*"),
            ("<b>b</b> <i>i</i> <del>gone</del>", "**b** *i* ~~gone~~"),
            ("line one<br>line two", "line one\nline two"),
            ("<p>para one</p><p>para two</p>", "para one\npara two"),
            (
                "use <code>fmt.Println</code> here",
                "use `fmt.Println` here",
            ),
            (
                r#"<a href="https://example.org/x">docs</a>"#,
                "[docs](https://example.org/x)",
            ),
            (
                r#"<a href="https://example.org">https://example.org</a>"#,
                "https://example.org",
            ),
            ("<span>&lt;tag&gt; &amp; more</span>", "<tag> & more"),
            ("<ul><li>one</li><li>two</li></ul>", "- one\n- two"),
        ];
        for (input, want) in cases {
            assert_eq!(html_to_markdown(input), want, "input {input:?}");
        }
    }

    #[test]
    fn html_pre_block_wraps_the_inline_code() {
        assert_eq!(
            html_to_markdown("<pre><code>x := 1</code></pre>"),
            "```\n`x := 1`\n```"
        );
    }

    #[test]
    fn html_unescape_handles_numeric_and_unknown_references() {
        assert_eq!(html_unescape("&#65;&#x42;&amp;"), "AB&");
        assert_eq!(html_unescape("a &bogus; b & c"), "a &bogus; b & c");
        assert_eq!(
            html_unescape("&#xD800;"),
            "&#xD800;",
            "surrogates are not characters"
        );
    }

    // ---- timestamps ----

    #[test]
    fn parses_rfc3339_timestamps() {
        assert_eq!(ts_ms("1970-01-01T00:00:00Z"), 0);
        assert_eq!(ts_ms("2024-05-01T12:34:56.789Z"), 1_714_566_896_789);
        assert_eq!(ts_ms("2024-05-01T14:34:56.789+02:00"), 1_714_566_896_789);
        assert_eq!(ts_ms("2026-08-31T09:14:02.000Z"), 1_788_167_642_000);
        assert_eq!(ts_ms("2024-05-01T12:34:56.7Z"), 1_714_566_896_700);
        assert_eq!(ts_ms(""), 0);
        assert_eq!(ts_ms("garbage"), 0);
        assert_eq!(ts_ms("2024-13-01T00:00:00Z"), 0);
    }

    // ---- chatinfo ----

    #[test]
    fn guild_channel_is_a_child_of_its_space() {
        let ch = Channel {
            id: "10".to_owned(),
            kind: CHANNEL_GUILD_TEXT,
            name: Some("general".to_owned()),
            ..Default::default()
        };
        let meta = channel_meta(
            &ch,
            Some(Parent {
                id: "9",
                name: "Space",
                icon: Some("abc"),
            }),
            "100",
            "https://media.example.test",
        )
        .expect("a text channel is a chat");
        assert_eq!(meta.name, "general");
        assert!(!meta.is_dm);
        assert_eq!(meta.portal.receiver, "");
        assert_eq!(meta.parent.as_ref().map(|p| p.id.as_str()), Some("9"));
        assert_eq!(meta.parent_name, "Space");
        assert_eq!(
            meta.avatar_url,
            "https://media.example.test/icons/9/abc.webp"
        );
    }

    #[test]
    fn direct_chat_belongs_to_the_login_and_names_the_partner() {
        let ch = Channel {
            id: "555".to_owned(),
            kind: CHANNEL_DM,
            recipients: vec![User {
                id: "9".to_owned(),
                username: "ada".to_owned(),
                global_name: Some("Ada".to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let meta = channel_meta(&ch, None, "100", "https://media.example.test").expect("dm");
        assert!(meta.is_dm);
        assert_eq!(meta.name, "Ada");
        assert_eq!(meta.portal.receiver, "100");
        assert_eq!(meta.dm_user.as_ref().map(|u| u.id.as_str()), Some("9"));
    }

    #[test]
    fn voice_and_category_channels_are_not_chats() {
        let voice = Channel {
            id: "2".to_owned(),
            kind: 2,
            ..Default::default()
        };
        assert!(channel_meta(&voice, None, "100", "").is_none());
        let space = Guild {
            id: "5".to_owned(),
            name: "Space".to_owned(),
            icon: None,
        };
        assert!(guild_meta(&space, "").is_space);
    }

    #[test]
    fn image_urls_use_gif_for_animated_hashes() {
        assert_eq!(
            image_url("https://media.example.test/", "/avatars/1/", "a_hash"),
            "https://media.example.test/avatars/1/a_hash.gif"
        );
        assert_eq!(
            image_url("https://media.example.test", "avatars/1", "hash"),
            "https://media.example.test/avatars/1/hash.webp"
        );
    }

    // ---- partial payloads parse ----

    #[test]
    fn partial_payloads_parse_with_defaults() {
        let m: Message = serde_json::from_str(r#"{"id":"1"}"#).expect("partial message");
        assert_eq!(m.id, "1");
        assert!(m.attachments.is_empty());
        let ready: Ready = serde_json::from_str("{}").expect("empty READY");
        assert!(ready.guilds.is_empty());
        let guild: GuildState =
            serde_json::from_str(r#"{"id":"9","unavailable":true}"#).expect("partial guild");
        assert!(guild.unavailable && guild.channels.is_empty());
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let m: Message = serde_json::from_str(
            r#"{"id":"1","channel_id":"2","new_field":{"x":1},"author":{"id":"3","extra":true}}"#,
        )
        .expect("message with unknown fields");
        assert_eq!(m.author.id, "3");
    }
}
