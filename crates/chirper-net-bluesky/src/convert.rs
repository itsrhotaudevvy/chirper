//! Remote → domain conversion: profiles, chats, messages, reactions and rich text. Everything here
//! is a pure function of the responses, so each rule is pinned by a unit test.
//!
//! Rich text: Bluesky facets address the UTF-8 bytes of the text. A facet whose offsets are out of
//! range or split a character is dropped, never trusted.

use std::collections::HashMap;

use serde_json::{Value, json};

use chirper_bridge::model::{ChatMeta, Content, PortalRef, Reaction, RemoteMessage, RemoteUser};
use chirper_bridge::{Error, Result};

use crate::api::chat::Message;
use crate::api::types::{ConvoView, DeletedMessageView, MessageView, Profile};
use crate::ids;

/// The users known to the account, by DID.
pub type Users = HashMap<String, RemoteUser>;

/// The text shown for a message that was deleted, in a history page.
const DELETED_TEXT: &str = "Deleted message";
const LINK_FEATURE: &str = "app.bsky.richtext.facet#link";
const MENTION_FEATURE: &str = "app.bsky.richtext.facet#mention";

/// Who is reading: the account's DID, and the users it knows.
#[derive(Clone, Copy, Debug)]
pub struct Viewer<'a> {
    pub did: &'a str,
    pub users: &'a Users,
}

/// The display name of a profile: its display name, or its handle when that is empty.
pub fn display_name(profile: &Profile) -> String {
    profile
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(&profile.handle)
        .to_owned()
}

/// A profile as a remote user.
pub fn user_of_profile(profile: &Profile) -> Result<RemoteUser> {
    Ok(RemoteUser {
        id: ids::user_id(&profile.did)?,
        display_name: display_name(profile),
        avatar_url: profile.avatar.clone().unwrap_or_default(),
        username: profile.handle.clone(),
        phone: String::new(),
    })
}

/// The remote user of a DID: the known profile, or the bare id when nothing is known.
pub fn user_for(did: &str, users: &Users) -> Result<RemoteUser> {
    match users.get(did) {
        Some(user) => Ok(user.clone()),
        None => Ok(RemoteUser {
            id: ids::user_id(did)?,
            ..RemoteUser::default()
        }),
    }
}

/// The portal of a conversation, keyed by the account's login as the Go bridge did.
pub fn portal(convo: &str, did: &str) -> PortalRef {
    PortalRef {
        id: convo.to_owned(),
        receiver: did.to_owned(),
    }
}

/// A conversation as chat metadata. Two members make a direct message; its name is the other
/// person. A group is named after its other members.
pub fn chat_meta(convo: &ConvoView, own_did: &str) -> Result<ChatMeta> {
    let others: Vec<&Profile> = convo
        .members
        .iter()
        .filter(|member| member.did != own_did)
        .collect();
    let is_dm = convo.members.len() == 2;
    let dm_user = match (is_dm, others.first()) {
        (true, Some(other)) => Some(user_of_profile(other)?),
        _ => None,
    };
    let name = others
        .iter()
        .map(|member| display_name(member))
        .collect::<Vec<_>>()
        .join(", ");
    Ok(ChatMeta {
        portal: portal(&convo.id, own_did),
        name: if name.is_empty() {
            convo.id.clone()
        } else {
            name
        },
        avatar_url: dm_user
            .as_ref()
            .map(|user| user.avatar_url.clone())
            .unwrap_or_default(),
        is_dm,
        member_count: i64::try_from(convo.members.len()).unwrap_or(i64::MAX),
        dm_user,
        ..ChatMeta::default()
    })
}

/// The unix time in milliseconds of an RFC 3339 timestamp.
pub fn timestamp_ms(sent_at: &str) -> Result<i64> {
    chrono::DateTime::parse_from_rfc3339(sent_at)
        .map(|time| time.timestamp_millis())
        .map_err(|err| Error::Protocol(format!("Bluesky timestamp: {err}")))
}

/// A message that still exists, as a text message. `live` is true for events from the log.
pub fn text_message(
    convo: &str,
    view: &MessageView,
    viewer: &Viewer<'_>,
    live: bool,
) -> Result<RemoteMessage> {
    let (body, formatted) = render(&view.text, &view.facets);
    Ok(RemoteMessage {
        portal: portal(convo, viewer.did),
        id: ids::message_id(convo, &view.id),
        part: String::new(),
        sender: user_for(&view.sender.did, viewer.users)?,
        timestamp_ms: timestamp_ms(&view.sent_at)?,
        content: Content::Text { body, formatted },
        reply_to: None,
        edit_of: None,
        live,
        from_me: view.sender.did == viewer.did,
    })
}

/// A deleted message, shown as a notice in history.
pub fn deleted_message(
    convo: &str,
    view: &DeletedMessageView,
    viewer: &Viewer<'_>,
) -> Result<RemoteMessage> {
    Ok(RemoteMessage {
        portal: portal(convo, viewer.did),
        id: ids::message_id(convo, &view.id),
        part: String::new(),
        sender: user_for(&view.sender.did, viewer.users)?,
        timestamp_ms: timestamp_ms(&view.sent_at)?,
        content: Content::Notice {
            body: DELETED_TEXT.to_owned(),
        },
        reply_to: None,
        edit_of: None,
        live: false,
        from_me: view.sender.did == viewer.did,
    })
}

/// A history entry as a message. Unknown entries give `None`.
pub fn history_message(
    convo: &str,
    message: &Message,
    viewer: &Viewer<'_>,
) -> Result<Option<RemoteMessage>> {
    match message {
        Message::View(view) => text_message(convo, view, viewer, false).map(Some),
        Message::Deleted(view) => deleted_message(convo, view, viewer).map(Some),
        Message::Other => Ok(None),
    }
}

/// The full reaction set of a message: one entry per emoji, with its reactors.
pub fn reactions(view: &MessageView, viewer: &Viewer<'_>) -> Result<Vec<Reaction>> {
    let mut out: Vec<Reaction> = Vec::new();
    for reaction in &view.reactions {
        let user = user_for(&reaction.sender.did, viewer.users)?;
        let from_me = reaction.sender.did == viewer.did;
        match out.iter_mut().find(|known| known.key == reaction.value) {
            Some(known) => {
                known.count += 1;
                known.from_me |= from_me;
                known.senders.push(user);
            }
            None => out.push(Reaction {
                key: reaction.value.clone(),
                count: 1,
                from_me,
                senders: vec![user],
            }),
        }
    }
    Ok(out)
}

/// A span of the text a rich-text feature applies to. Byte offsets, valid char boundaries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: SpanKind,
}

/// What a span is. A mention holds a DID in a received message and a handle in a draft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpanKind {
    Link(String),
    Mention(String),
}

/// The text of `text` between two byte offsets, or empty when they are not valid offsets.
fn slice(text: &str, start: usize, end: usize) -> &str {
    text.get(start..end).unwrap_or_default()
}

/// The spans of received facets: valid, sorted, and never overlapping.
pub fn incoming_spans(text: &str, facets: &[Value]) -> Vec<Span> {
    let mut spans: Vec<Span> = facets
        .iter()
        .filter_map(|facet| facet_span(text, facet))
        .collect();
    spans.sort_by_key(|span| span.start);
    let mut kept: Vec<Span> = Vec::with_capacity(spans.len());
    for span in spans {
        if kept.last().is_none_or(|prev| span.start >= prev.end) {
            kept.push(span);
        }
    }
    kept
}

fn facet_span(text: &str, facet: &Value) -> Option<Span> {
    let index = facet.get("index")?;
    let start = usize::try_from(index.get("byteStart")?.as_u64()?).ok()?;
    let end = usize::try_from(index.get("byteEnd")?.as_u64()?).ok()?;
    let in_range = start < end
        && end <= text.len()
        && text.is_char_boundary(start)
        && text.is_char_boundary(end);
    if !in_range {
        return None;
    }
    let kind = facet
        .get("features")?
        .as_array()?
        .iter()
        .find_map(feature_kind)?;
    Some(Span { start, end, kind })
}

fn feature_kind(feature: &Value) -> Option<SpanKind> {
    match feature.get("$type")?.as_str()? {
        LINK_FEATURE => Some(SpanKind::Link(feature.get("uri")?.as_str()?.to_owned())),
        MENTION_FEATURE => Some(SpanKind::Mention(feature.get("did")?.as_str()?.to_owned())),
        _ => None,
    }
}

/// Only web links become anchors; any other link is shown as plain text.
fn is_web_link(uri: &str) -> bool {
    (uri.starts_with("https://") && uri.len() > 8) || (uri.starts_with("http://") && uri.len() > 7)
}

/// The plain body of a received message and, when it has web links, the HTML subset that marks them.
pub fn render(text: &str, facets: &[Value]) -> (String, Option<String>) {
    let links: Vec<Span> = incoming_spans(text, facets)
        .into_iter()
        .filter(|span| matches!(&span.kind, SpanKind::Link(uri) if is_web_link(uri)))
        .collect();
    if links.is_empty() {
        return (text.to_owned(), None);
    }
    let mut html = String::new();
    let mut at = 0;
    for span in &links {
        let SpanKind::Link(uri) = &span.kind else {
            continue;
        };
        html.push_str(&escape(slice(text, at, span.start)));
        html.push_str("<a href=\"");
        html.push_str(&escape(uri));
        html.push_str("\">");
        html.push_str(&escape(slice(text, span.start, span.end)));
        html.push_str("</a>");
        at = span.end;
    }
    html.push_str(&escape(slice(text, at, text.len())));
    (text.to_owned(), Some(html))
}

/// Escape the characters that are markup in HTML text and attribute values.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// The spans to send with a draft: web links, and `@handle` mentions of a dotted handle. Their
/// handles are not resolved here.
pub fn detect(text: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut word_start: Option<usize> = None;
    for (at, ch) in text.char_indices() {
        if ch.is_whitespace() {
            if let Some(start) = word_start.take() {
                spans.extend(word_span(text, start, at));
            }
        } else if word_start.is_none() {
            word_start = Some(at);
        }
    }
    if let Some(start) = word_start {
        spans.extend(word_span(text, start, text.len()));
    }
    spans
}

fn word_span(text: &str, start: usize, end: usize) -> Option<Span> {
    let word = slice(text, start, end).trim_end_matches(TRAILING_PUNCTUATION);
    let end = start + word.len();
    if let Some(rest) = word.strip_prefix('@') {
        return is_handle(rest).then(|| Span {
            start,
            end,
            kind: SpanKind::Mention(rest.to_owned()),
        });
    }
    if is_web_link(word) {
        return Some(Span {
            start,
            end,
            kind: SpanKind::Link(word.to_owned()),
        });
    }
    None
}

const TRAILING_PUNCTUATION: &[char] = &['.', ',', ';', ':', '!', '?', ')', ']', '}', '\'', '"'];

/// A dotted handle: letters, digits, `-` and `.`, with at least one dot.
fn is_handle(text: &str) -> bool {
    text.contains('.')
        && text.len() <= 253
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        && !text.starts_with('.')
        && !text.ends_with('.')
}

/// The facet JSON of a span: its byte range and one feature.
pub fn facet_json(start: usize, end: usize, feature: Value) -> Value {
    json!({"index": {"byteStart": start, "byteEnd": end}, "features": [feature]})
}

/// The link feature of a facet.
pub fn link_feature(uri: &str) -> Value {
    json!({"$type": LINK_FEATURE, "uri": uri})
}

/// The mention feature of a facet.
pub fn mention_feature(did: &str) -> Value {
    json!({"$type": MENTION_FEATURE, "did": did})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(start: usize, end: usize, uri: &str) -> Value {
        facet_json(start, end, link_feature(uri))
    }

    fn profile(did: &str, handle: &str, name: Option<&str>) -> Profile {
        Profile {
            did: did.to_owned(),
            handle: handle.to_owned(),
            display_name: name.map(str::to_owned),
            avatar: None,
        }
    }

    #[test]
    fn a_display_name_falls_back_to_the_handle() {
        assert_eq!(
            display_name(&profile("did:plc:a", "ada.test", Some("Ada"))),
            "Ada"
        );
        assert_eq!(
            display_name(&profile("did:plc:a", "ada.test", Some("  "))),
            "ada.test"
        );
        assert_eq!(
            display_name(&profile("did:plc:a", "ada.test", None)),
            "ada.test"
        );
    }

    #[test]
    fn a_profile_becomes_a_user_with_the_bridge_id() {
        let user = user_of_profile(&profile("did:plc:abc", "ada.test", Some("Ada"))).unwrap();
        assert_eq!(user.id, "plc-abc");
        assert_eq!(user.display_name, "Ada");
        assert_eq!(user.username, "ada.test");
    }

    #[test]
    fn a_two_member_conversation_is_a_direct_message_named_after_the_other_person() {
        let convo = ConvoView {
            id: "c1".to_owned(),
            members: vec![
                profile("did:plc:me", "me.test", None),
                profile("did:plc:bob", "bob.test", Some("Bob")),
            ],
        };
        let meta = chat_meta(&convo, "did:plc:me").unwrap();
        assert!(meta.is_dm);
        assert_eq!(meta.name, "Bob");
        assert_eq!(meta.member_count, 2);
        assert_eq!(meta.portal, portal("c1", "did:plc:me"));
        assert_eq!(meta.dm_user.map(|u| u.id).as_deref(), Some("plc-bob"));
    }

    #[test]
    fn a_group_is_named_after_its_other_members() {
        let convo = ConvoView {
            id: "g1".to_owned(),
            members: vec![
                profile("did:plc:me", "me.test", None),
                profile("did:plc:bob", "bob.test", None),
                profile("did:plc:cy", "cy.test", Some("Cy")),
            ],
        };
        let meta = chat_meta(&convo, "did:plc:me").unwrap();
        assert!(!meta.is_dm);
        assert_eq!(meta.name, "bob.test, Cy");
        assert!(meta.dm_user.is_none());
    }

    #[test]
    fn timestamps_are_unix_milliseconds() {
        assert_eq!(
            timestamp_ms("2026-01-01T00:00:00.123Z").unwrap(),
            1_767_225_600_123
        );
        assert!(matches!(timestamp_ms("yesterday"), Err(Error::Protocol(_))));
    }

    #[test]
    fn a_text_message_uses_the_message_id_and_knows_its_own_messages() {
        let users = Users::new();
        let viewer = Viewer {
            did: "did:plc:me",
            users: &users,
        };
        let view = MessageView {
            id: "m1".to_owned(),
            text: "hi".to_owned(),
            facets: Vec::new(),
            reactions: Vec::new(),
            sender: crate::api::types::Sender {
                did: "did:plc:me".to_owned(),
            },
            sent_at: "2026-01-01T00:00:00.000Z".to_owned(),
        };
        let message = text_message("c1", &view, &viewer, true).unwrap();
        assert_eq!(message.id, "c1:m1");
        assert_eq!(message.sender.id, "plc-me");
        assert!(message.from_me);
        assert!(message.live);
        assert_eq!(message.content.body(), "hi");
    }

    #[test]
    fn a_deleted_message_is_a_notice_in_history() {
        let users = Users::new();
        let viewer = Viewer {
            did: "did:plc:me",
            users: &users,
        };
        let view = DeletedMessageView {
            id: "m9".to_owned(),
            sender: crate::api::types::Sender {
                did: "did:plc:bob".to_owned(),
            },
            sent_at: "2026-01-01T00:00:00.000Z".to_owned(),
        };
        let message = deleted_message("c1", &view, &viewer).unwrap();
        assert!(matches!(&message.content, Content::Notice { body } if body == DELETED_TEXT));
        assert!(!message.live);
        assert!(!message.from_me);
    }

    #[test]
    fn reactions_group_by_emoji_and_count_their_reactors() {
        use crate::api::types::{ReactionView, Sender};
        let users = Users::new();
        let viewer = Viewer {
            did: "did:plc:me",
            users: &users,
        };
        let reaction = |value: &str, who: &str| ReactionView {
            value: value.to_owned(),
            sender: Sender {
                did: who.to_owned(),
            },
        };
        let view = MessageView {
            id: "m1".to_owned(),
            text: String::new(),
            facets: Vec::new(),
            reactions: vec![
                reaction("👍", "did:plc:bob"),
                reaction("🎉", "did:plc:bob"),
                reaction("👍", "did:plc:me"),
            ],
            sender: Sender {
                did: "did:plc:bob".to_owned(),
            },
            sent_at: "2026-01-01T00:00:00.000Z".to_owned(),
        };
        let out = reactions(&view, &viewer).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].key, "👍");
        assert_eq!(out[0].count, 2);
        assert!(out[0].from_me);
        assert_eq!(out[1].count, 1);
        assert!(!out[1].from_me);
    }

    #[test]
    fn a_web_link_facet_becomes_an_anchor_and_text_is_escaped() {
        let text = "see <b> https://example.com/a?x=1&y=2 now";
        let start = text.find("https").unwrap();
        let end = start + "https://example.com/a?x=1&y=2".len();
        let (body, html) = render(text, &[link(start, end, "https://example.com/a?x=1&y=2")]);
        assert_eq!(body, text);
        assert_eq!(
            html.unwrap(),
            "see &lt;b&gt; <a href=\"https://example.com/a?x=1&amp;y=2\">https://example.com/a?x=1&amp;y=2</a> now"
        );
    }

    #[test]
    fn byte_offsets_after_emoji_are_bytes_not_characters() {
        let text = "👋 https://x.test ok";
        let start = text.find("https").unwrap();
        assert_eq!(start, 5, "the wave is four bytes");
        let end = start + "https://x.test".len();
        let (_, html) = render(text, &[link(start, end, "https://x.test")]);
        assert_eq!(
            html.unwrap(),
            "👋 <a href=\"https://x.test\">https://x.test</a> ok"
        );
    }

    #[test]
    fn facets_that_split_a_character_or_leave_the_text_are_dropped() {
        let text = "👋 ok";
        assert!(incoming_spans(text, &[link(1, 3, "https://x.test")]).is_empty());
        assert!(incoming_spans(text, &[link(0, 99, "https://x.test")]).is_empty());
        assert!(incoming_spans(text, &[link(3, 3, "https://x.test")]).is_empty());
        assert!(
            incoming_spans(text, &[json!({"index": {"byteStart": -1, "byteEnd": 2}})]).is_empty()
        );
        assert!(incoming_spans(text, &[json!({"features": "nope"})]).is_empty());
    }

    #[test]
    fn overlapping_facets_keep_the_first_one() {
        let text = "abcdefgh";
        let spans = incoming_spans(
            text,
            &[link(0, 5, "https://a.test"), link(3, 7, "https://b.test")],
        );
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].end, 5);
    }

    #[test]
    fn non_web_links_are_plain_text() {
        let text = "bad link";
        let (body, html) = render(text, &[link(0, 3, "javascript:alert(1)")]);
        assert_eq!(body, text);
        assert!(html.is_none());
    }

    #[test]
    fn mentions_are_not_rendered_but_stay_as_text() {
        let text = "hi @ada.test";
        let facet = facet_json(3, 12, mention_feature("did:plc:ada"));
        let (body, html) = render(text, &[facet]);
        assert_eq!(body, text);
        assert!(html.is_none());
    }

    #[test]
    fn a_draft_yields_link_and_mention_spans_with_byte_offsets() {
        let text = "café https://x.test/é, and @ada.test!";
        let spans = detect(text);
        assert_eq!(spans.len(), 2);
        let link_span = &spans[0];
        assert_eq!(
            slice(text, link_span.start, link_span.end),
            "https://x.test/é"
        );
        assert_eq!(
            link_span.kind,
            SpanKind::Link("https://x.test/é".to_owned())
        );
        let mention = &spans[1];
        assert_eq!(slice(text, mention.start, mention.end), "@ada.test");
        assert_eq!(mention.kind, SpanKind::Mention("ada.test".to_owned()));
    }

    #[test]
    fn words_that_are_not_links_or_handles_are_not_spans() {
        assert!(detect("@nodot hello http:// @.bad.").is_empty());
    }

    #[test]
    fn facet_json_has_the_wire_shape() {
        let facet = facet_json(1, 4, mention_feature("did:plc:x"));
        assert_eq!(facet["index"]["byteStart"], 1);
        assert_eq!(facet["index"]["byteEnd"], 4);
        assert_eq!(facet["features"][0]["$type"], MENTION_FEATURE);
    }

    #[test]
    fn history_skips_unknown_entries() {
        let users = Users::new();
        let viewer = Viewer {
            did: "did:plc:me",
            users: &users,
        };
        assert!(
            history_message("c1", &Message::Other, &viewer)
                .unwrap()
                .is_none()
        );
    }
}
