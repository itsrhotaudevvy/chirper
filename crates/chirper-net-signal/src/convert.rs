//! Remote content to bridge events: text and formatting, replies, edits, deletions, reactions,
//! attachments and the other message kinds.
//!
//! Ported from signalmeow's `receiving.go` and `connector/handlesignal.go` (AGPL-3.0) and the
//! formatting rules of `msgconv/from-signal.go`. The functions here are pure: attachments are
//! already downloaded and decrypted by the caller.

use std::collections::BTreeMap;

use chirper_bridge::model::{Content, MediaInfo, Reaction, RemoteEventRef, RemoteMessage, RemoteUser};
use chirper_bridge::{Event, PortalRef};

use crate::ids;
use crate::proto::{BodyRange, DataMessage, body_range, body_style};

/// Bold, italic, strikethrough and monospace, as bit flags over UTF-16 units.
const BOLD: u8 = 1;
const ITALIC: u8 = 2;
const STRIKE: u8 = 4;
const MONO: u8 = 8;

/// One attachment after download and decryption, as the caller hands it over.
#[derive(Debug, Clone)]
pub struct Media {
    /// `image`, `video`, `audio`, `voice`, `file` or `sticker`.
    pub kind: String,
    pub info: MediaInfo,
    /// The plaintext, or `None` when the file could not be fetched or verified.
    pub data: Option<Vec<u8>>,
}

/// Where one message came from.
#[derive(Debug, Clone)]
pub struct Origin {
    /// The local account's ACI.
    pub me: String,
    /// The ACI of the sender (our own ACI for messages sent from another device).
    pub sender: String,
    /// True when the local user sent it from another device.
    pub from_me: bool,
    /// The envelope's timestamp, used when the message has none of its own.
    pub fallback_ts: u64,
}

/// The reactions on every message, as the full set per target. Reactions arrive one at a time, so
/// the book keeps each reactor's emoji and reports the whole set after every change.
#[derive(Debug, Default, Clone)]
pub struct ReactionBook {
    targets: BTreeMap<String, BTreeMap<String, String>>,
}

impl ReactionBook {
    /// Applies one reaction (or its removal) and returns the full set for `target`.
    pub fn apply(&mut self, me: &str, target: &str, reactor: &str, emoji: &str, remove: bool) -> Vec<Reaction> {
        let reactors = self.targets.entry(target.to_owned()).or_default();
        if remove {
            if reactors.get(reactor).is_some_and(|current| current == emoji) {
                reactors.remove(reactor);
            }
        } else {
            reactors.insert(reactor.to_owned(), emoji.to_owned());
        }
        let mut by_emoji: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (reactor, emoji) in reactors.iter() {
            by_emoji.entry(emoji.as_str()).or_default().push(reactor.as_str());
        }
        by_emoji
            .into_iter()
            .map(|(emoji, reactors)| Reaction {
                key: emoji.to_owned(),
                count: i64::try_from(reactors.len()).unwrap_or(i64::MAX),
                from_me: reactors.contains(&me),
                senders: reactors
                    .iter()
                    .map(|id| RemoteUser {
                        id: ids::user_id(id),
                        ..RemoteUser::default()
                    })
                    .collect(),
            })
            .collect()
    }
}

/// The remote user for an ACI.
pub fn user(aci: &str) -> RemoteUser {
    RemoteUser {
        id: ids::user_id(aci),
        ..RemoteUser::default()
    }
}

/// Converts the events of one `DataMessage` (or an edit of one) in the chat `portal`.
pub fn data_events(
    origin: &Origin,
    portal: &PortalRef,
    data: &DataMessage,
    edit_target: Option<u64>,
    media: Vec<Media>,
    reactions: &mut ReactionBook,
) -> Vec<Event> {
    let timestamp = data.timestamp.filter(|t| *t > 0).unwrap_or(origin.fallback_ts);
    if let Some(reaction) = &data.reaction {
        return reaction_event(origin, portal, reaction, reactions);
    }
    if let Some(delete) = &data.delete {
        let target = delete.target_sent_timestamp.unwrap_or(0);
        return vec![redact(portal, &ids::message_id(&origin.sender, target))];
    }
    if let Some(admin) = &data.admin_delete {
        let author = admin
            .target_author_aci_binary
            .as_deref()
            .and_then(|b| ids::uuid_from_bytes(b).ok())
            .unwrap_or_else(|| origin.sender.clone());
        let target = admin.target_sent_timestamp.unwrap_or(0);
        return vec![redact(portal, &ids::message_id(&author, target))];
    }
    let reply_to = data.quote.as_ref().and_then(|quote| {
        let author = quote
            .author_aci
            .clone()
            .or_else(|| quote.author_aci_binary.as_deref().and_then(|b| ids::uuid_from_bytes(b).ok()))?;
        Some(ids::message_id(&author, quote.id.unwrap_or(0)))
    });
    let edit_of = edit_target.map(|target| ids::message_id(&origin.sender, target));
    let body = data.body.clone().unwrap_or_default();
    let has_media = !media.is_empty();
    let mut messages = Vec::new();
    if has_media {
        for (index, item) in media.into_iter().enumerate() {
            let caption = if index == 0 { body.clone() } else { String::new() };
            let content = match item.data {
                Some(bytes) => Content::Media {
                    kind: item.kind.clone(),
                    caption,
                    info: item.info.clone(),
                    data: std::sync::Arc::new(bytes),
                    thumbnail: None,
                },
                None => Content::Unsupported {
                    body: format!("{} could not be downloaded", describe_media(&item.kind)),
                },
            };
            messages.push((ids::part_id(index), content));
        }
    } else if !body.is_empty() {
        let formatted = formatted_body(&body, &data.body_ranges);
        messages.push((
            String::new(),
            Content::Text {
                body,
                formatted,
            },
        ));
    } else if let Some(sticker) = &data.sticker {
        let description = sticker.emoji.clone().unwrap_or_default();
        messages.push((
            String::new(),
            Content::Unsupported {
                body: format!("sticker {description}").trim().to_owned(),
            },
        ));
    } else if data.flags.unwrap_or(0) & crate::proto::data_flags::EXPIRATION_TIMER_UPDATE != 0
        || data.flags.unwrap_or(0) & crate::proto::data_flags::PROFILE_KEY_UPDATE != 0
    {
        return Vec::new();
    } else {
        messages.push((
            String::new(),
            Content::Unsupported {
                body: "A message Chirper cannot show yet".to_owned(),
            },
        ));
    }
    messages
        .into_iter()
        .map(|(part, content)| {
            Event::Message(RemoteMessage {
                portal: portal.clone(),
                id: ids::message_id(&origin.sender, timestamp),
                part,
                sender: user(&origin.sender),
                timestamp_ms: i64::try_from(timestamp).unwrap_or(i64::MAX),
                content,
                reply_to: reply_to.clone(),
                edit_of: edit_of.clone(),
                live: true,
                from_me: origin.from_me,
            })
        })
        .collect()
}

/// The event of a reaction or a reaction removal.
fn reaction_event(
    origin: &Origin,
    portal: &PortalRef,
    reaction: &crate::proto::Reaction,
    reactions: &mut ReactionBook,
) -> Vec<Event> {
    let target_author = reaction
        .target_author_aci_binary
        .as_deref()
        .and_then(|b| ids::uuid_from_bytes(b).ok())
        .or_else(|| reaction.target_author_aci.clone())
        .unwrap_or_else(|| origin.sender.clone());
    let target_ts = reaction.target_sent_timestamp.unwrap_or(0);
    let target = ids::message_id(&target_author, target_ts);
    let emoji = reaction.emoji.clone().unwrap_or_default();
    let set = reactions.apply(
        &origin.me,
        &target,
        &origin.sender,
        &emoji,
        reaction.remove.unwrap_or(false),
    );
    vec![Event::Reactions {
        portal: portal.clone(),
        target: RemoteEventRef::new(target),
        reactions: set,
    }]
}

fn redact(portal: &PortalRef, target: &str) -> Event {
    Event::Redact {
        portal: portal.clone(),
        event: RemoteEventRef::new(target),
    }
}

fn describe_media(kind: &str) -> &'static str {
    match kind {
        "image" => "The image",
        "video" => "The video",
        "audio" => "The audio",
        "voice" => "The voice message",
        "sticker" => "The sticker",
        _ => "The file",
    }
}

/// The HTML subset (`b i s code`) for the body and its formatting ranges. `None` when the message
/// has no styling, so the plain text stands alone. Ranges are in UTF-16 code units, as Signal
/// sends them; the body is escaped.
pub fn formatted_body(body: &str, ranges: &[BodyRange]) -> Option<String> {
    let units: Vec<u16> = body.encode_utf16().collect();
    let mut flags = vec![0u8; units.len()];
    let mut any = false;
    for range in ranges {
        let Some(style) = range.associated.as_ref().and_then(|a| match a {
            body_range::Associated::Style(s) => Some(*s),
            _ => None,
        }) else {
            continue;
        };
        let bit = match style {
            body_style::BOLD => BOLD,
            body_style::ITALIC => ITALIC,
            body_style::STRIKETHROUGH => STRIKE,
            body_style::MONOSPACE => MONO,
            _ => continue,
        };
        let start = usize::try_from(range.start.unwrap_or(0)).unwrap_or(usize::MAX);
        let length = usize::try_from(range.length.unwrap_or(0)).unwrap_or(0);
        let end = start.saturating_add(length).min(units.len());
        for flag in flags.iter_mut().take(end).skip(start) {
            *flag |= bit;
        }
        any = true;
    }
    if !any {
        return None;
    }
    let mut html = String::new();
    let mut open = 0u8;
    for (index, unit) in units.iter().enumerate() {
        let wanted = flags.get(index).copied().unwrap_or(0);
        if wanted != open {
            html.push_str(&close_tags(open));
            html.push_str(&open_tags(wanted));
            open = wanted;
        }
        let ch = char::from_u32(u32::from(*unit)).unwrap_or('\u{fffd}');
        push_escaped(&mut html, ch);
    }
    html.push_str(&close_tags(open));
    Some(html)
}

fn open_tags(flags: u8) -> String {
    let mut out = String::new();
    if flags & BOLD != 0 {
        out.push_str("<b>");
    }
    if flags & ITALIC != 0 {
        out.push_str("<i>");
    }
    if flags & STRIKE != 0 {
        out.push_str("<s>");
    }
    if flags & MONO != 0 {
        out.push_str("<code>");
    }
    out
}

fn close_tags(flags: u8) -> String {
    let mut out = String::new();
    if flags & MONO != 0 {
        out.push_str("</code>");
    }
    if flags & STRIKE != 0 {
        out.push_str("</s>");
    }
    if flags & ITALIC != 0 {
        out.push_str("</i>");
    }
    if flags & BOLD != 0 {
        out.push_str("</b>");
    }
    out
}

fn push_escaped(out: &mut String, ch: char) {
    match ch {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '"' => out.push_str("&quot;"),
        '\n' => out.push_str("<br>"),
        other => out.push(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: &str = "a1b2c3d4-e5f6-4789-8abc-def012345678";
    const ALICE: &str = "11111111-2222-4333-8444-555555555555";

    fn origin() -> Origin {
        Origin {
            me: ME.to_owned(),
            sender: ALICE.to_owned(),
            from_me: false,
            fallback_ts: 99,
        }
    }

    fn portal() -> PortalRef {
        PortalRef {
            id: ids::dm_portal_id(ALICE),
            receiver: String::new(),
        }
    }

    fn style(start: u32, length: u32, style: i32) -> BodyRange {
        BodyRange {
            start: Some(start),
            length: Some(length),
            associated: Some(body_range::Associated::Style(style)),
        }
    }

    #[test]
    fn a_plain_text_message_keeps_its_id_and_timestamp() {
        let data = DataMessage {
            body: Some("hello".to_owned()),
            timestamp: Some(1_700_000_000_123),
            ..DataMessage::default()
        };
        let mut book = ReactionBook::default();
        let events = data_events(&origin(), &portal(), &data, None, Vec::new(), &mut book);
        assert_eq!(events.len(), 1);
        let Event::Message(message) = &events[0] else {
            panic!("expected a message");
        };
        assert_eq!(message.id, format!("{ALICE}|1700000000123"));
        assert_eq!(message.part, "");
        assert_eq!(message.timestamp_ms, 1_700_000_000_123);
        assert!(matches!(&message.content, Content::Text { body, formatted: None } if body == "hello"));
        assert!(message.live && !message.from_me);
    }

    #[test]
    fn body_ranges_become_html_over_utf16_units() {
        // "héllo 😀 bold" with "bold" at UTF-16 offset 8 (the emoji counts as two units).
        let body = "héllo 😀 bold";
        let html = formatted_body(body, &[style(9, 4, body_style::BOLD)]).expect("styled");
        assert_eq!(html, "héllo 😀 <b>bold</b>");
        let escaped = formatted_body("<a>", &[style(0, 3, body_style::ITALIC)]).expect("styled");
        assert_eq!(escaped, "<i>&lt;a&gt;</i>");
        assert!(formatted_body("plain", &[]).is_none());
    }

    #[test]
    fn a_reply_points_at_the_quoted_message() {
        let data = DataMessage {
            body: Some("re".to_owned()),
            timestamp: Some(5),
            quote: Some(crate::proto::Quote {
                id: Some(42),
                author_aci: Some(ME.to_owned()),
                ..crate::proto::Quote::default()
            }),
            ..DataMessage::default()
        };
        let mut book = ReactionBook::default();
        let events = data_events(&origin(), &portal(), &data, None, Vec::new(), &mut book);
        let Event::Message(message) = &events[0] else {
            panic!("expected a message");
        };
        assert_eq!(message.reply_to.as_deref(), Some(format!("{ME}|42").as_str()));
    }

    #[test]
    fn an_edit_targets_the_original_message_id() {
        let data = DataMessage {
            body: Some("fixed".to_owned()),
            timestamp: Some(200),
            ..DataMessage::default()
        };
        let mut book = ReactionBook::default();
        let events = data_events(&origin(), &portal(), &data, Some(100), Vec::new(), &mut book);
        let Event::Message(message) = &events[0] else {
            panic!("expected a message");
        };
        assert_eq!(message.edit_of.as_deref(), Some(format!("{ALICE}|100").as_str()));
        assert_eq!(message.id, format!("{ALICE}|200"));
    }

    #[test]
    fn a_delete_redacts_the_target_and_a_reaction_reports_the_full_set() {
        let mut book = ReactionBook::default();
        let delete = DataMessage {
            delete: Some(crate::proto::Delete {
                target_sent_timestamp: Some(7),
            }),
            ..DataMessage::default()
        };
        let events = data_events(&origin(), &portal(), &delete, None, Vec::new(), &mut book);
        assert!(matches!(&events[0], Event::Redact { event, .. } if event.id == format!("{ALICE}|7")));

        let react = |emoji: &str, remove: bool| DataMessage {
            reaction: Some(crate::proto::Reaction {
                emoji: Some(emoji.to_owned()),
                remove: Some(remove),
                target_author_aci: Some(ME.to_owned()),
                target_sent_timestamp: Some(9),
                ..crate::proto::Reaction::default()
            }),
            ..DataMessage::default()
        };
        let first = data_events(&origin(), &portal(), &react("👍", false), None, Vec::new(), &mut book);
        let Event::Reactions { reactions, target, .. } = &first[0] else {
            panic!("expected reactions");
        };
        assert_eq!(target.id, format!("{ME}|9"));
        assert_eq!(reactions.len(), 1);
        assert_eq!(reactions[0].count, 1);
        let removed = data_events(&origin(), &portal(), &react("👍", true), None, Vec::new(), &mut book);
        let Event::Reactions { reactions, .. } = &removed[0] else {
            panic!("expected reactions");
        };
        assert!(reactions.is_empty());
    }

    #[test]
    fn a_failed_attachment_becomes_an_unsupported_part() {
        let data = DataMessage {
            body: Some("look".to_owned()),
            timestamp: Some(3),
            ..DataMessage::default()
        };
        let media = vec![Media {
            kind: "image".to_owned(),
            info: MediaInfo::default(),
            data: None,
        }];
        let mut book = ReactionBook::default();
        let events = data_events(&origin(), &portal(), &data, None, media, &mut book);
        let Event::Message(message) = &events[0] else {
            panic!("expected a message");
        };
        assert!(matches!(&message.content, Content::Unsupported { body } if body.contains("could not be downloaded")));
    }
}
