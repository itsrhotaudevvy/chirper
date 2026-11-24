//! X's shapes → the bridge's domain types: chats, users, text with its entities, media, and the
//! reaction set of a message. Reference: `connector/msgconv.go`, `connector/chatsync.go`,
//! `connector/chatinfo.go` and `connector/twitterfmt/convert.go`.

use std::collections::HashMap;
use std::sync::Arc;

use chirper_bridge::model::{ChatMeta, Content, MediaInfo, PortalRef, Reaction, RemoteUser};

use crate::api::model::{Conversation, MediaInfo as XMedia, MessageData, User, Variant};
use crate::ids;

/// Largest attachment the driver downloads; bigger files are described instead.
pub const MAX_MEDIA_BYTES: u64 = 100 * 1024 * 1024;

/// The user with this id, from the cache the inbox filled. Unknown users keep only their id.
pub fn remote_user(id: &str, users: &HashMap<String, User>) -> RemoteUser {
    match users.get(id) {
        Some(user) => RemoteUser {
            id: id.to_owned(),
            display_name: if user.name.is_empty() {
                user.screen_name.clone()
            } else {
                user.name.clone()
            },
            avatar_url: user.profile_image_url_https.clone(),
            username: user.screen_name.clone(),
            phone: String::new(),
        },
        None => RemoteUser {
            id: id.to_owned(),
            display_name: id.to_owned(),
            ..Default::default()
        },
    }
}

/// The portal of a conversation id, as the bridge stores it.
pub fn portal(conversation_id: &str, me_id: &str) -> PortalRef {
    let id = ids::portal_id(conversation_id);
    let receiver = ids::portal_receiver(&id, me_id);
    PortalRef { id, receiver }
}

/// The members of a one-to-one conversation other than the local user. A conversation with
/// yourself has you as its only member, so the list falls back to the ids in its own id.
fn dm_partner(conversation: &Conversation, me_id: &str) -> String {
    let other = conversation
        .participants
        .iter()
        .map(|p| p.user_id.as_str())
        .find(|id| *id != me_id && !id.is_empty());
    if let Some(other) = other {
        return other.to_owned();
    }
    conversation
        .conversation_id
        .split(':')
        .find(|id| *id != me_id && !id.is_empty())
        .unwrap_or(me_id)
        .to_owned()
}

/// The chat metadata of a conversation: a group's name and avatar, or a direct message named after
/// the other person.
pub fn chat_meta(
    conversation: &Conversation,
    users: &HashMap<String, User>,
    me_id: &str,
) -> ChatMeta {
    let portal = portal(&conversation.conversation_id, me_id);
    if conversation.is_group() {
        let avatar = if conversation.avatar.image.original_info.url.is_empty() {
            conversation.avatar_image_https.clone()
        } else {
            conversation.avatar.image.original_info.url.clone()
        };
        return ChatMeta {
            portal,
            name: conversation.name.clone(),
            avatar_url: avatar,
            is_dm: false,
            member_count: conversation.participants.len() as i64,
            ..Default::default()
        };
    }
    let partner = dm_partner(conversation, me_id);
    let user = remote_user(&partner, users);
    ChatMeta {
        portal,
        name: user.display_name.clone(),
        avatar_url: user.avatar_url.clone(),
        is_dm: true,
        member_count: 2,
        dm_user: Some(user),
        ..Default::default()
    }
}

/// The time of a message in Unix milliseconds: the stated time, or the snowflake's when it is
/// missing.
pub fn timestamp_ms(time: &str, id: &str) -> i64 {
    match time.parse::<i64>() {
        Ok(ms) if ms > 0 => ms,
        _ => ids::snowflake_ms(id),
    }
}

/// The text of a message as `(body, formatted)`. Entities are applied the way the reference does:
/// a URL shows its expanded address, and a mention links to the account. Formatted HTML is only
/// produced when the message has entities.
pub fn text_of(data: &MessageData) -> (String, Option<String>) {
    let Some(entities) = &data.entities else {
        return (data.text.clone(), None);
    };
    let chars: Vec<char> = data.text.chars().collect();
    let mut spans: Vec<Span> = Vec::new();
    for url in &entities.urls {
        if let [start, end, ..] = url.indices[..] {
            let expanded = if url.expanded_url.is_empty() {
                url.url.clone()
            } else {
                url.expanded_url.clone()
            };
            let shown = if url.display_url.is_empty() {
                expanded.clone()
            } else {
                url.display_url.clone()
            };
            spans.push(Span {
                start,
                end,
                body: expanded.clone(),
                html: format!("<a href=\"{}\">{}</a>", escape(&expanded), escape(&shown)),
            });
        }
    }
    for mention in &entities.user_mentions {
        if let [start, end, ..] = mention.indices[..] {
            let shown: String = chars
                .get(start..end.min(chars.len()))
                .unwrap_or_default()
                .iter()
                .collect();
            let target = if mention.screen_name.is_empty() {
                String::new()
            } else {
                format!("https://x.com/{}", mention.screen_name)
            };
            let html = if target.is_empty() {
                escape(&shown)
            } else {
                format!("<a href=\"{}\">{}</a>", escape(&target), escape(&shown))
            };
            spans.push(Span {
                start,
                end,
                body: shown,
                html,
            });
        }
    }
    spans.sort_by_key(|span| span.start);
    let mut body = String::new();
    let mut html = String::new();
    let mut cursor = 0usize;
    for span in spans {
        if span.start < cursor || span.end > chars.len() || span.start > span.end {
            continue;
        }
        let before: String = chars[cursor..span.start].iter().collect();
        body.push_str(&before);
        html.push_str(&escape(&before));
        body.push_str(&span.body);
        html.push_str(&span.html);
        cursor = span.end;
    }
    let rest: String = chars[cursor..].iter().collect();
    body.push_str(&rest);
    html.push_str(&escape(&rest));
    (body, Some(html.replace('\n', "<br>")))
}

/// A text span replaced by its own body and HTML.
struct Span {
    start: usize,
    end: usize,
    body: String,
    html: String,
}

/// Escape the characters that are special in HTML.
pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The media a message carries: what to download and how to describe it.
pub struct MediaSource {
    pub kind: &'static str,
    pub mime: &'static str,
    pub url: String,
    pub width: i64,
    pub height: i64,
    pub duration_ms: i64,
}

/// The first attachment of a message that the driver can download.
pub fn media_of(data: &MessageData) -> Option<MediaSource> {
    let attachment = data.attachment.as_ref();
    let from_attachment = attachment.and_then(|a| {
        a.photo
            .as_ref()
            .map(|m| (m, "photo"))
            .or_else(|| a.video.as_ref().map(|m| (m, "video")))
            .or_else(|| a.animated_gif.as_ref().map(|m| (m, "animated_gif")))
            .or_else(|| a.media.as_ref().map(|m| (m, "")))
    });
    let from_entities = data
        .entities
        .as_ref()
        .and_then(|e| e.media.first())
        .map(|m| (m, ""));
    let (info, hint) = from_attachment.or(from_entities)?;
    let kind = if hint.is_empty() {
        info.kind.as_str()
    } else {
        hint
    };
    let source = match kind {
        "photo" => image_source(info, "image/jpeg"),
        "animated_gif" | "gif" => image_source(info, "image/gif"),
        "video" | "audio" => video_source(info),
        _ if !info.video_info.variants.is_empty() || info.video_info.duration_millis > 0 => {
            video_source(info)
        }
        _ => image_source(info, "image/jpeg"),
    }?;
    Some(source)
}

fn image_source(info: &XMedia, mime: &'static str) -> Option<MediaSource> {
    let url = first_url(&[
        &info.media_url_https,
        &info.media_url,
        &info.expanded_url,
        &info.url,
    ])?;
    Some(MediaSource {
        kind: "image",
        mime,
        url,
        width: info.original_info.width,
        height: info.original_info.height,
        duration_ms: 0,
    })
}

fn video_source(info: &XMedia) -> Option<MediaSource> {
    let variant = highest_bitrate(&info.video_info.variants).map(|v| v.url.as_str());
    let url = first_url(&[
        variant.unwrap_or_default(),
        &info.media_url_https,
        &info.media_url,
        &info.expanded_url,
        &info.url,
    ])?;
    Some(MediaSource {
        kind: "video",
        mime: "video/mp4",
        url,
        width: info.original_info.width,
        height: info.original_info.height,
        duration_ms: info.video_info.duration_millis,
    })
}

/// The mp4 variant with the highest bitrate, the one the reference picks.
fn highest_bitrate(variants: &[Variant]) -> Option<&Variant> {
    variants
        .iter()
        .filter(|v| !v.url.is_empty())
        .max_by_key(|v| v.bitrate)
}

/// The first candidate that is an https URL on a host the driver may fetch from.
fn first_url(candidates: &[&str]) -> Option<String> {
    candidates
        .iter()
        .map(|c| normalize_url(c))
        .find(|u| !u.is_empty())
}

/// Protocol-relative and scheme-less media addresses become https.
pub fn normalize_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Some(rest) = trimmed.strip_prefix("//") {
        return format!("https://{rest}");
    }
    if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
        return trimmed.to_owned();
    }
    if trimmed.contains('.') {
        return format!("https://{trimmed}");
    }
    String::new()
}

/// The file name a download is saved under, from its address.
pub fn file_name(url: &str, mime: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let last = path.rsplit('/').next().unwrap_or_default();
    if !last.is_empty() && last.contains('.') {
        return last.to_owned();
    }
    let extension = match mime {
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "video/mp4" => "mp4",
        _ => "bin",
    };
    format!("attachment.{extension}")
}

/// The bridge's content for a message whose media is already downloaded.
pub fn media_content(source: &MediaSource, caption: String, data: Vec<u8>) -> Content {
    let size = data.len();
    Content::Media {
        kind: source.kind.to_owned(),
        caption,
        info: MediaInfo {
            mime: source.mime.to_owned(),
            file_name: file_name(&source.url, source.mime),
            size,
            width: source.width,
            height: source.height,
            duration_ms: source.duration_ms,
            blurhash: String::new(),
        },
        data: Arc::new(data),
        thumbnail: None,
    }
}

/// The reactions of one message, grouped by emoji, with who reacted.
pub fn reaction_summary(
    reactors: &[(String, String)],
    users: &HashMap<String, User>,
    me_id: &str,
) -> Vec<Reaction> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<String>> = HashMap::new();
    for (emoji, sender) in reactors {
        if !groups.contains_key(emoji) {
            order.push(emoji.clone());
        }
        groups
            .entry(emoji.clone())
            .or_default()
            .push(sender.clone());
    }
    order
        .into_iter()
        .filter_map(|emoji| {
            let senders = groups.remove(&emoji)?;
            Some(Reaction {
                key: emoji,
                count: senders.len() as i64,
                from_me: senders.iter().any(|s| s == me_id),
                senders: senders.iter().map(|s| remote_user(s, users)).collect(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::model::{Entities, UrlEntity};

    fn data(text: &str) -> MessageData {
        MessageData {
            text: text.to_owned(),
            ..Default::default()
        }
    }

    #[test]
    fn plain_text_has_no_formatted_body() {
        assert_eq!(text_of(&data("hi <b>")), ("hi <b>".to_owned(), None));
    }

    #[test]
    fn a_url_entity_shows_its_expanded_address_and_links_it() {
        let mut message = data("see t.co/x now");
        message.entities = Some(Entities {
            urls: vec![UrlEntity {
                url: "https://t.co/x".into(),
                expanded_url: "https://example.org/page".into(),
                display_url: "example.org/page".into(),
                indices: vec![4, 10],
            }],
            ..Default::default()
        });
        let (body, html) = text_of(&message);
        assert_eq!(body, "see https://example.org/page now");
        assert_eq!(
            html.as_deref(),
            Some("see <a href=\"https://example.org/page\">example.org/page</a> now")
        );
    }

    #[test]
    fn text_outside_entities_is_escaped_in_html() {
        let mut message = data("a < b & c");
        message.entities = Some(Entities::default());
        assert_eq!(text_of(&message).1.as_deref(), Some("a &lt; b &amp; c"));
    }

    #[test]
    fn a_protocol_relative_media_url_becomes_https() {
        assert_eq!(
            normalize_url("//pbs.twimg.com/a.jpg"),
            "https://pbs.twimg.com/a.jpg"
        );
        assert_eq!(normalize_url("   "), "");
        assert_eq!(normalize_url("nonsense"), "");
    }

    #[test]
    fn a_video_uses_its_highest_bitrate_variant() {
        let mut message = data("");
        message.attachment = Some(crate::api::model::Attachment {
            video: Some(XMedia {
                video_info: crate::api::model::VideoInfo {
                    duration_millis: 4000,
                    variants: vec![
                        Variant {
                            bitrate: 300,
                            content_type: "video/mp4".into(),
                            url: "https://video.twimg.com/low.mp4".into(),
                        },
                        Variant {
                            bitrate: 2000,
                            content_type: "video/mp4".into(),
                            url: "https://video.twimg.com/high.mp4".into(),
                        },
                    ],
                },
                ..Default::default()
            }),
            ..Default::default()
        });
        let source = media_of(&message).expect("a video is media");
        assert_eq!(source.kind, "video");
        assert_eq!(source.url, "https://video.twimg.com/high.mp4");
        assert_eq!(source.duration_ms, 4000);
    }

    #[test]
    fn a_file_name_comes_from_the_address_or_the_type() {
        assert_eq!(
            file_name(
                "https://pbs.twimg.com/media/abc.jpg?name=orig",
                "image/jpeg"
            ),
            "abc.jpg"
        );
        assert_eq!(
            file_name("https://pbs.twimg.com/media/abc", "image/gif"),
            "attachment.gif"
        );
    }

    #[test]
    fn reactions_group_by_emoji_and_mark_the_local_user() {
        let users = HashMap::new();
        let reactors = vec![
            ("🔥".to_owned(), "1".to_owned()),
            ("🔥".to_owned(), "2".to_owned()),
            ("👍".to_owned(), "1".to_owned()),
        ];
        let summary = reaction_summary(&reactors, &users, "1");
        assert_eq!(summary.len(), 2);
        assert_eq!(summary[0].key, "🔥");
        assert_eq!(summary[0].count, 2);
        assert!(summary[0].from_me);
        assert!(!summary[1].from_me || summary[1].count == 1);
    }

    #[test]
    fn a_direct_message_is_named_after_the_other_person() {
        let mut users = HashMap::new();
        users.insert(
            "222".to_owned(),
            User {
                id_str: "222".into(),
                name: "Ada".into(),
                screen_name: "ada".into(),
                profile_image_url_https: "https://pbs.twimg.com/a.jpg".into(),
            },
        );
        let conversation = Conversation {
            conversation_id: "111:222".into(),
            kind: "ONE_TO_ONE".into(),
            participants: vec![
                crate::api::model::Participant {
                    user_id: "111".into(),
                    ..Default::default()
                },
                crate::api::model::Participant {
                    user_id: "222".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let meta = chat_meta(&conversation, &users, "111");
        assert_eq!(meta.portal.id, "111-222");
        assert_eq!(meta.portal.receiver, "111");
        assert_eq!(meta.name, "Ada");
        assert!(meta.is_dm);
    }
}
