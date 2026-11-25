//! Conversion between protocol messages and the bridge's content model, in both directions.
//!
//! Incoming: [`classify`] unwraps the wrapper messages (view-once, ephemeral, captions, edits) and
//! names what the content is. Media is described, not downloaded, so the caller can fetch it.
//! Outgoing: the builders make the protocol messages for each bridge command.
//!
//! WhatsApp's own formatting (`*bold*`, `_italic_`, `~strike~`, triple-backtick code) becomes the
//! HTML subset the bridge shows: `b i s pre` with escaped text.

use crate::binary::Jid;
use crate::ids::{self, ParsedMessageId};
use crate::proto::e2e::{ContextInfo, Message, ReactionMessage};
use crate::proto::{MessageKey, e2e};

/// A media attachment as the protocol describes it. The bytes are not here yet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MediaSpec {
    /// `image`, `video`, `audio`, `voice`, `file`, `sticker`.
    pub kind: &'static str,
    /// The WhatsApp media type, which selects the HKDF info string: `Image`, `Video`, `Audio`,
    /// `Document` or `Sticker` (stickers use the image keys).
    pub app_info: &'static str,
    /// The WhatsApp `mediatype` label the uploads and downloads use.
    pub mms_type: &'static str,
    pub mime: String,
    pub file_name: String,
    pub caption: String,
    pub width: i64,
    pub height: i64,
    pub duration_ms: i64,
    pub size: u64,
    pub direct_path: String,
    pub media_key: Vec<u8>,
    pub file_enc_sha256: Vec<u8>,
    pub file_sha256: Vec<u8>,
    pub thumbnail: Option<Vec<u8>>,
}

/// What a message holds, as the driver understands it.
#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    /// Text, with the formatted HTML when WhatsApp formatting was present.
    Text {
        body: String,
        formatted: Option<String>,
        reply_to: Option<ReplyTo>,
    },
    Media {
        spec: Box<MediaSpec>,
        reply_to: Option<ReplyTo>,
    },
    Location(String),
    Contact(String),
    Unsupported(String),
    /// A message that carries nothing to show (a key share, an empty wrapper).
    Empty,
}

/// The message a reply quotes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplyTo {
    /// The quoted message's stanza id.
    pub stanza_id: String,
    /// The quoted message's sender, when the server gave one.
    pub participant: String,
}

/// Unwraps the wrapper messages and names what is left.
pub fn classify(message: &Message) -> Body {
    if let Some(inner) = unwrap_wrapper(message) {
        return classify(inner);
    }
    if let Some(text) = &message.conversation {
        return Body::Text {
            body: text.clone(),
            formatted: format_whatsapp(text),
            reply_to: None,
        };
    }
    if let Some(ext) = &message.extended_text_message {
        let text = ext.text.clone().unwrap_or_default();
        return Body::Text {
            formatted: format_whatsapp(&text),
            body: text,
            reply_to: ext.context_info.as_ref().and_then(reply_of),
        };
    }
    if let Some(image) = &message.image_message {
        return media(
            MediaSpec {
                kind: "image",
                app_info: "Image",
                mms_type: "image",
                mime: image.mimetype.clone().unwrap_or_default(),
                caption: image.caption.clone().unwrap_or_default(),
                width: i64::from(image.width.unwrap_or(0)),
                height: i64::from(image.height.unwrap_or(0)),
                size: image.file_length.unwrap_or(0),
                direct_path: image.direct_path.clone().unwrap_or_default(),
                media_key: image.media_key.clone().unwrap_or_default(),
                file_enc_sha256: image.file_enc_sha256.clone().unwrap_or_default(),
                file_sha256: image.file_sha256.clone().unwrap_or_default(),
                thumbnail: image.jpeg_thumbnail.clone(),
                ..Default::default()
            },
            image.context_info.as_ref(),
        );
    }
    if let Some(video) = &message.video_message {
        return video_body(video);
    }
    if let Some(video) = &message.ptv_message {
        return video_body(video);
    }
    if let Some(audio) = &message.audio_message {
        let voice = audio.ptt.unwrap_or(false);
        return media(
            MediaSpec {
                kind: if voice { "voice" } else { "audio" },
                app_info: "Audio",
                mms_type: if voice { "ptt" } else { "audio" },
                mime: audio.mimetype.clone().unwrap_or_default(),
                duration_ms: i64::from(audio.seconds.unwrap_or(0)) * 1000,
                size: audio.file_length.unwrap_or(0),
                direct_path: audio.direct_path.clone().unwrap_or_default(),
                media_key: audio.media_key.clone().unwrap_or_default(),
                file_enc_sha256: audio.file_enc_sha256.clone().unwrap_or_default(),
                file_sha256: audio.file_sha256.clone().unwrap_or_default(),
                ..Default::default()
            },
            audio.context_info.as_ref(),
        );
    }
    if let Some(document) = &message.document_message {
        return media(
            MediaSpec {
                kind: "file",
                app_info: "Document",
                mms_type: "document",
                mime: document.mimetype.clone().unwrap_or_default(),
                file_name: document.file_name.clone().unwrap_or_default(),
                caption: document.caption.clone().unwrap_or_default(),
                size: document.file_length.unwrap_or(0),
                direct_path: document.direct_path.clone().unwrap_or_default(),
                media_key: document.media_key.clone().unwrap_or_default(),
                file_enc_sha256: document.file_enc_sha256.clone().unwrap_or_default(),
                file_sha256: document.file_sha256.clone().unwrap_or_default(),
                thumbnail: document.jpeg_thumbnail.clone(),
                ..Default::default()
            },
            document.context_info.as_ref(),
        );
    }
    if let Some(sticker) = &message.sticker_message {
        return media(
            MediaSpec {
                kind: "sticker",
                app_info: "Sticker",
                mms_type: "image",
                mime: sticker
                    .mimetype
                    .clone()
                    .unwrap_or_else(|| "image/webp".to_owned()),
                width: i64::from(sticker.width.unwrap_or(0)),
                height: i64::from(sticker.height.unwrap_or(0)),
                size: sticker.file_length.unwrap_or(0),
                direct_path: sticker.direct_path.clone().unwrap_or_default(),
                media_key: sticker.media_key.clone().unwrap_or_default(),
                file_enc_sha256: sticker.file_enc_sha256.clone().unwrap_or_default(),
                file_sha256: sticker.file_sha256.clone().unwrap_or_default(),
                ..Default::default()
            },
            sticker.context_info.as_ref(),
        );
    }
    if let Some(location) = &message.location_message {
        let name = location.name.clone().unwrap_or_default();
        let address = location.address.clone().unwrap_or_default();
        let coordinates = format!(
            "{:.6}, {:.6}",
            location.degrees_latitude.unwrap_or(0.0),
            location.degrees_longitude.unwrap_or(0.0)
        );
        let mut text = format!("Location: {coordinates}");
        if !name.is_empty() {
            text = format!("{name}\n{text}");
        }
        if !address.is_empty() {
            text = format!("{text}\n{address}");
        }
        return Body::Location(text);
    }
    if let Some(contact) = &message.contact_message {
        return Body::Contact(format!(
            "Contact: {}",
            contact.display_name.clone().unwrap_or_default()
        ));
    }
    if message.poll_creation_message.is_some()
        || message.poll_creation_message_v2.is_some()
        || message.poll_creation_message_v3.is_some()
    {
        let poll = message
            .poll_creation_message
            .as_ref()
            .or(message.poll_creation_message_v2.as_ref())
            .or(message.poll_creation_message_v3.as_ref())
            .and_then(|poll| poll.name.clone())
            .unwrap_or_default();
        return Body::Unsupported(format!("Poll: {poll}"));
    }
    if message.lottie_sticker_message.is_some() {
        return Body::Unsupported("Animated sticker".to_owned());
    }
    Body::Empty
}

fn media(spec: MediaSpec, context: Option<&ContextInfo>) -> Body {
    Body::Media {
        spec: Box::new(spec),
        reply_to: context.and_then(reply_of),
    }
}

fn video_body(video: &e2e::VideoMessage) -> Body {
    media(
        MediaSpec {
            kind: "video",
            app_info: "Video",
            mms_type: "video",
            mime: video.mimetype.clone().unwrap_or_default(),
            caption: video.caption.clone().unwrap_or_default(),
            width: i64::from(video.width.unwrap_or(0)),
            height: i64::from(video.height.unwrap_or(0)),
            duration_ms: i64::from(video.seconds.unwrap_or(0)) * 1000,
            size: video.file_length.unwrap_or(0),
            direct_path: video.direct_path.clone().unwrap_or_default(),
            media_key: video.media_key.clone().unwrap_or_default(),
            file_enc_sha256: video.file_enc_sha256.clone().unwrap_or_default(),
            file_sha256: video.file_sha256.clone().unwrap_or_default(),
            thumbnail: video.jpeg_thumbnail.clone(),
            ..Default::default()
        },
        video.context_info.as_ref(),
    )
}

/// The message inside a wrapper that carries content on its own (view-once, ephemeral, a caption
/// on a document, an edit). `None` for an ordinary message.
pub fn unwrap_wrapper(message: &Message) -> Option<&Message> {
    let inner = [
        &message.view_once_message,
        &message.view_once_message_v2,
        &message.view_once_message_v2_extension,
        &message.ephemeral_message,
        &message.document_with_caption_message,
        &message.edited_message,
        &message.lottie_sticker_message,
    ]
    .into_iter()
    .find_map(|wrapper| wrapper.as_ref())?;
    inner.message.as_deref()
}

/// The reply context of a message, when it quotes another one.
pub fn reply_of(context: &ContextInfo) -> Option<ReplyTo> {
    let stanza_id = context.stanza_id.clone().filter(|id| !id.is_empty())?;
    Some(ReplyTo {
        stanza_id,
        participant: context.participant.clone().unwrap_or_default(),
    })
}

/// The remote id of a quoted message, given the chat it was sent in.
pub fn reply_message_id(chat: &Jid, reply: &ReplyTo) -> Option<String> {
    let sender: Jid = if reply.participant.is_empty() {
        chat.clone()
    } else {
        reply.participant.parse().ok()?
    };
    Some(ids::message_id(chat, &sender, &reply.stanza_id))
}

/// WhatsApp's formatting as the HTML subset the bridge shows. Returns `None` when the text has no
/// formatting, so the plain body is used alone.
pub fn format_whatsapp(text: &str) -> Option<String> {
    let html = format_inline(text);
    if html == escape(text) {
        None
    } else {
        Some(html)
    }
}

fn format_inline(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(stripped) = rest.strip_prefix("```")
            && let Some(end) = stripped.find("```")
        {
            out.push_str("<pre>");
            out.push_str(&escape(&stripped[..end]));
            out.push_str("</pre>");
            rest = &stripped[end + 3..];
            continue;
        }
        let Some(marker) = rest.chars().next() else {
            break;
        };
        if let Some((tag, width)) = tag_for(marker)
            && let Some(end) = closing(rest, marker)
        {
            let inner = &rest[width..end];
            out.push_str(&format!("<{tag}>{}</{tag}>", escape(inner)));
            rest = &rest[end + width..];
            continue;
        }
        let len = marker.len_utf8();
        out.push_str(&escape(&rest[..len]));
        rest = &rest[len..];
    }
    out
}

fn tag_for(marker: char) -> Option<(&'static str, usize)> {
    match marker {
        '*' => Some(("b", 1)),
        '_' => Some(("i", 1)),
        '~' => Some(("s", 1)),
        _ => None,
    }
}

/// The byte index of the closing `marker` of a span that opens at the start of `text`, if the
/// span is well formed: non-space after the opener, non-space before the closer, and the span
/// is closed on the same line.
fn closing(text: &str, marker: char) -> Option<usize> {
    let width = marker.len_utf8();
    let after = text[width..].chars().next()?;
    if after.is_whitespace() {
        return None;
    }
    let mut search = width;
    while let Some(offset) = text[search..].find(marker) {
        let at = search + offset;
        let before = text[..at].chars().next_back()?;
        let inner_ok = !before.is_whitespace() && at > width;
        let after_close = text[at + width..].chars().next();
        let closes = after_close.is_none_or(|c| c.is_whitespace() || c.is_ascii_punctuation());
        if inner_ok && closes {
            return Some(at);
        }
        if text[search..at].contains('\n') {
            return None;
        }
        search = at + width;
    }
    None
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// The key of a message in `chat`: the form WhatsApp uses to refer to one. `from_me` says whether
/// the message is ours; in a group the participant names its sender.
pub fn key_for(chat: &Jid, target: &ParsedMessageId, from_me: bool) -> MessageKey {
    MessageKey {
        remote_jid: Some(ids::portal_id(chat)),
        from_me: Some(from_me),
        id: Some(target.id.clone()),
        participant: (chat.server == crate::binary::Server::Group)
            .then(|| ids::portal_id(&target.sender)),
    }
}

/// A plain text message, with a reply context when `reply_to` names a message.
pub fn text_message(text: &str, reply_to: Option<&str>) -> Message {
    match reply_to.and_then(ids::parse_message_id) {
        None => Message {
            conversation: Some(text.to_owned()),
            ..Default::default()
        },
        Some(target) => Message {
            extended_text_message: Some(e2e::ExtendedTextMessage {
                text: Some(text.to_owned()),
                context_info: Some(ContextInfo {
                    stanza_id: Some(target.id),
                    participant: Some(ids::portal_id(&target.sender)),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        },
    }
}

/// An edit of one of our messages: a `MESSAGE_EDIT` protocol message carrying the new text.
pub fn edit_message(chat: &Jid, target: &ParsedMessageId, text: &str, own: &Jid) -> Message {
    let from_me = target.sender.user == own.user;
    Message {
        protocol_message: Some(e2e::ProtocolMessage {
            key: Some(key_for(chat, target, from_me)),
            r#type: Some(e2e::protocol_type::MESSAGE_EDIT),
            edited_message: Some(Box::new(text_message(text, None))),
            timestamp_ms: Some(now_ms()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// A revoke of a message: sent by its author (`from_me`) or by an admin of the group.
pub fn revoke_message(chat: &Jid, target: &ParsedMessageId, from_me: bool) -> Message {
    Message {
        protocol_message: Some(e2e::ProtocolMessage {
            key: Some(key_for(chat, target, from_me)),
            r#type: Some(e2e::protocol_type::REVOKE),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// A reaction to a message. An empty emoji removes ours.
pub fn reaction_message(chat: &Jid, target: &ParsedMessageId, own: &Jid, emoji: &str) -> Message {
    let from_me = target.sender.user == own.user;
    Message {
        reaction_message: Some(ReactionMessage {
            key: Some(key_for(chat, target, from_me)),
            text: Some(emoji.to_owned()),
            sender_timestamp_ms: Some(now_ms()),
        }),
        ..Default::default()
    }
}

/// Current time in Unix milliseconds.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whatsapp_formatting_becomes_html() {
        assert_eq!(
            format_whatsapp("say *hi* and _bye_ ~now~").as_deref(),
            Some("say <b>hi</b> and <i>bye</i> <s>now</s>")
        );
        assert_eq!(
            format_whatsapp("```let x = 1 < 2```").as_deref(),
            Some("<pre>let x = 1 &lt; 2</pre>")
        );
    }

    #[test]
    fn plain_text_and_stray_markers_are_left_alone() {
        assert_eq!(format_whatsapp("just words"), None);
        assert_eq!(format_whatsapp("5 * 3 = 15"), None);
        assert_eq!(format_whatsapp("snake_case_name"), None);
    }

    #[test]
    fn html_in_text_is_escaped() {
        assert_eq!(
            format_whatsapp("<b>x</b> *y*").as_deref(),
            Some("&lt;b&gt;x&lt;/b&gt; <b>y</b>")
        );
    }

    #[test]
    fn conversation_and_extended_text_classify_as_text() {
        let plain = Message {
            conversation: Some("hello".to_owned()),
            ..Default::default()
        };
        assert_eq!(
            classify(&plain),
            Body::Text {
                body: "hello".to_owned(),
                formatted: None,
                reply_to: None
            }
        );
        let reply = Message {
            extended_text_message: Some(e2e::ExtendedTextMessage {
                text: Some("re".to_owned()),
                context_info: Some(ContextInfo {
                    stanza_id: Some("ABC".to_owned()),
                    participant: Some("555@s.whatsapp.net".to_owned()),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            classify(&reply),
            Body::Text {
                body: "re".to_owned(),
                formatted: None,
                reply_to: Some(ReplyTo {
                    stanza_id: "ABC".to_owned(),
                    participant: "555@s.whatsapp.net".to_owned()
                })
            }
        );
    }

    #[test]
    fn wrappers_are_unwrapped() {
        let inner = Message {
            conversation: Some("secret".to_owned()),
            ..Default::default()
        };
        let wrapped = Message {
            view_once_message: Some(e2e::FutureProofMessage {
                message: Some(Box::new(inner)),
            }),
            ..Default::default()
        };
        assert!(matches!(classify(&wrapped), Body::Text { body, .. } if body == "secret"));
    }

    #[test]
    fn voice_notes_and_images_name_their_kinds() {
        let voice = Message {
            audio_message: Some(e2e::AudioMessage {
                ptt: Some(true),
                seconds: Some(3),
                ..Default::default()
            }),
            ..Default::default()
        };
        match classify(&voice) {
            Body::Media { spec, .. } => {
                assert_eq!(spec.kind, "voice");
                assert_eq!(spec.duration_ms, 3000);
            }
            other => panic!("not media: {other:?}"),
        }
        let image = Message {
            image_message: Some(e2e::ImageMessage {
                caption: Some("a cat".to_owned()),
                width: Some(4),
                ..Default::default()
            }),
            ..Default::default()
        };
        match classify(&image) {
            Body::Media { spec, .. } => {
                assert_eq!(spec.kind, "image");
                assert_eq!(spec.caption, "a cat");
                assert_eq!(spec.app_info, "Image");
            }
            other => panic!("not media: {other:?}"),
        }
    }

    #[test]
    fn contacts_and_polls_are_described() {
        let contact = Message {
            contact_message: Some(e2e::ContactMessage {
                display_name: Some("Ada".to_owned()),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(classify(&contact), Body::Contact("Contact: Ada".to_owned()));
        let poll = Message {
            poll_creation_message: Some(e2e::PollCreationMessage {
                name: Some("Lunch?".to_owned()),
            }),
            ..Default::default()
        };
        assert_eq!(
            classify(&poll),
            Body::Unsupported("Poll: Lunch?".to_owned())
        );
        assert_eq!(classify(&Message::default()), Body::Empty);
    }

    #[test]
    fn reply_ids_resolve_to_the_quoted_sender() {
        let chat: Jid = "1-2@g.us".parse().expect("jid");
        let reply = ReplyTo {
            stanza_id: "3EB0X".to_owned(),
            participant: "555@s.whatsapp.net".to_owned(),
        };
        assert_eq!(
            reply_message_id(&chat, &reply).as_deref(),
            Some("1-2@g.us:555@s.whatsapp.net:3EB0X")
        );
    }

    #[test]
    fn text_builders_set_the_expected_fields() {
        let plain = text_message("hi", None);
        assert_eq!(plain.conversation.as_deref(), Some("hi"));
        let target = "1-2@g.us:555@s.whatsapp.net:3EB0X";
        let reply = text_message("re", Some(target));
        let context = reply
            .extended_text_message
            .and_then(|ext| ext.context_info)
            .expect("context");
        assert_eq!(context.stanza_id.as_deref(), Some("3EB0X"));
        assert_eq!(context.participant.as_deref(), Some("555@s.whatsapp.net"));
    }

    #[test]
    fn revokes_and_reactions_carry_the_target_key() {
        let chat: Jid = "1-2@g.us".parse().expect("jid");
        let target = ids::parse_message_id("1-2@g.us:555@s.whatsapp.net:3EB0X").expect("id");
        let own: Jid = "999@s.whatsapp.net".parse().expect("jid");
        let revoke = revoke_message(&chat, &target, false);
        let key = revoke.protocol_message.and_then(|p| p.key).expect("key");
        assert_eq!(key.remote_jid.as_deref(), Some("1-2@g.us"));
        assert_eq!(key.from_me, Some(false));
        assert_eq!(key.participant.as_deref(), Some("555@s.whatsapp.net"));
        let edit = edit_message(&chat, &target, "fixed", &own);
        let protocol = edit.protocol_message.expect("protocol");
        assert_eq!(protocol.r#type, Some(e2e::protocol_type::MESSAGE_EDIT));
        let react = reaction_message(&chat, &target, &own, "👍");
        let reaction = react.reaction_message.expect("reaction");
        assert_eq!(reaction.text.as_deref(), Some("👍"));
    }
}
