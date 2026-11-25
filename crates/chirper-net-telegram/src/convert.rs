//! Telegram objects to Chirper's model: entities to HTML, media to content (or a download to
//! perform), service messages to notices, reactions to the full set of a message.
//!
//! Pure functions over the TL types. The session fetches what [`Body::Download`] asks for.

use chirper_bridge::{Content, MediaInfo, Reaction};

use crate::format::{Span, Tag};
use crate::ids::{PeerKind, custom_emoji_key};
use crate::tl::{enums, types};

/// Largest file the driver downloads; bigger ones become an unsupported notice.
pub const MAX_MEDIA_BYTES: i64 = 100 * 1024 * 1024;

/// A media file the session has to download before the message can be emitted.
#[derive(Clone, Debug)]
pub struct Download {
    /// `image`, `video`, `audio`, `voice`, `file`, `sticker`.
    pub kind: String,
    pub caption: String,
    pub info: MediaInfo,
    pub location: enums::InputFileLocation,
    /// The data centre the file lives in.
    pub dc: i32,
}

/// The body of a message, as far as this module can say without network access.
#[derive(Clone, Debug)]
pub enum Body {
    Ready(Content),
    Download(Download),
}

/// A reply target: the peer (when the reply is in another chat), the message, and the topic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplyTarget {
    pub peer: Option<(PeerKind, i64)>,
    pub message: i32,
    pub topic: i32,
}

/// The kind and id of a peer.
pub fn peer_parts(peer: &enums::Peer) -> (PeerKind, i64) {
    match peer {
        enums::Peer::PeerUser(p) => (PeerKind::User, p.user_id),
        enums::Peer::PeerChat(p) => (PeerKind::Chat, p.chat_id),
        enums::Peer::PeerChannel(p) => (PeerKind::Channel, p.channel_id),
    }
}

/// The span list of a message's entities. Entities Chirper does not show are dropped.
pub fn spans(entities: &[enums::MessageEntity]) -> Vec<Span> {
    entities.iter().filter_map(span).collect()
}

fn span(entity: &enums::MessageEntity) -> Option<Span> {
    let (offset, length, tag) = match entity {
        enums::MessageEntity::MessageEntityBold(e) => (e.offset, e.length, Tag::Bold),
        enums::MessageEntity::MessageEntityItalic(e) => (e.offset, e.length, Tag::Italic),
        enums::MessageEntity::MessageEntityUnderline(e) => (e.offset, e.length, Tag::Underline),
        enums::MessageEntity::MessageEntityStrike(e) => (e.offset, e.length, Tag::Strike),
        enums::MessageEntity::MessageEntityCode(e) => (e.offset, e.length, Tag::Code),
        enums::MessageEntity::MessageEntityPre(e) => (
            e.offset,
            e.length,
            Tag::Pre {
                language: e.language.clone(),
            },
        ),
        enums::MessageEntity::MessageEntityTextUrl(e) => {
            (e.offset, e.length, Tag::Link { url: e.url.clone() })
        }
        enums::MessageEntity::MessageEntitySpoiler(e) => (e.offset, e.length, Tag::Spoiler),
        enums::MessageEntity::MessageEntityBlockquote(e) => (e.offset, e.length, Tag::Blockquote),
        _ => return None,
    };
    Some(Span {
        offset: i64::from(offset),
        length: i64::from(length),
        tag,
    })
}

/// A text body with its HTML rendering when it has any markup.
pub fn text_content(text: &str, entities: &[enums::MessageEntity]) -> Content {
    Content::Text {
        body: text.to_owned(),
        formatted: crate::format::to_html(text, &spans(entities)),
    }
}

/// The body of a message from its text and media. The caption of a media message is its text.
pub fn body(
    text: &str,
    entities: &[enums::MessageEntity],
    media: Option<&enums::MessageMedia>,
) -> Body {
    let Some(media) = media else {
        return Body::Ready(text_content(text, entities));
    };
    match media {
        enums::MessageMedia::MessageMediaEmpty(_) | enums::MessageMedia::MessageMediaWebPage(_) => {
            Body::Ready(text_content(text, entities))
        }
        enums::MessageMedia::MessageMediaPhoto(photo) => photo_body(text, photo.photo.as_deref()),
        enums::MessageMedia::MessageMediaDocument(document) => {
            document_body(text, document.document.as_deref())
        }
        enums::MessageMedia::MessageMediaGeo(geo) => location_body(&geo.geo),
        enums::MessageMedia::MessageMediaGeoLive(geo) => location_body(&geo.geo),
        enums::MessageMedia::MessageMediaVenue(venue) => location_body(&venue.geo),
        enums::MessageMedia::MessageMediaContact(_) => unsupported("a shared contact"),
        enums::MessageMedia::MessageMediaPoll(_) => unsupported("a poll"),
        enums::MessageMedia::MessageMediaDice(_) => unsupported("a dice roll"),
        enums::MessageMedia::MessageMediaGame(_) => unsupported("a game"),
        enums::MessageMedia::MessageMediaInvoice(_) => unsupported("an invoice"),
        _ => unsupported("this kind of message"),
    }
}

fn unsupported(what: &str) -> Body {
    Body::Ready(Content::Unsupported {
        body: format!("{what} is not supported by Chirper yet; open Telegram to view it"),
    })
}

fn location_body(geo: &enums::GeoPoint) -> Body {
    match geo {
        enums::GeoPoint::GeoPoint(point) => Body::Ready(Content::Location {
            body: format!("geo:{},{}", point.lat, point.long),
        }),
        enums::GeoPoint::GeoPointEmpty(_) => unsupported("an empty location"),
    }
}

/// The largest downloadable size of a photo: its type letter, dimensions and byte size.
fn best_photo_size(sizes: &[enums::PhotoSize]) -> Option<(String, i32, i32, i64)> {
    sizes
        .iter()
        .filter_map(|size| match size {
            enums::PhotoSize::PhotoSize(s) => Some((s.r#type.clone(), s.w, s.h, i64::from(s.size))),
            enums::PhotoSize::PhotoSizeProgressive(s) => {
                let bytes = s.sizes.iter().copied().max().unwrap_or(0);
                Some((s.r#type.clone(), s.w, s.h, i64::from(bytes)))
            }
            _ => None,
        })
        .max_by_key(|(_, _, _, bytes)| *bytes)
}

fn photo_body(text: &str, photo: Option<&enums::Photo>) -> Body {
    let Some(enums::Photo::Photo(photo)) = photo else {
        return unsupported("an expired photo");
    };
    let Some((kind, width, height, bytes)) = best_photo_size(&photo.sizes) else {
        return unsupported("a photo without a downloadable size");
    };
    if bytes > MAX_MEDIA_BYTES {
        return unsupported("a photo this large");
    }
    Body::Download(Download {
        kind: "image".into(),
        caption: text.to_owned(),
        info: MediaInfo {
            mime: "image/jpeg".into(),
            file_name: format!("photo-{}.jpg", photo.id),
            size: usize::try_from(bytes).unwrap_or(0),
            width: i64::from(width),
            height: i64::from(height),
            ..MediaInfo::default()
        },
        location: enums::InputFileLocation::InputPhotoFileLocation(types::InputPhotoFileLocation {
            id: photo.id,
            access_hash: photo.access_hash,
            file_reference: photo.file_reference.clone(),
            thumb_size: kind,
        }),
        dc: photo.dc_id,
    })
}

/// The media kind and the attributes Chirper shows, from a document's attributes.
struct DocumentFacts {
    kind: &'static str,
    file_name: Option<String>,
    width: i64,
    height: i64,
    duration_ms: i64,
}

fn document_facts(attributes: &[enums::DocumentAttribute], voice_flag: bool) -> DocumentFacts {
    let mut facts = DocumentFacts {
        kind: "file",
        file_name: None,
        width: 0,
        height: 0,
        duration_ms: 0,
    };
    for attribute in attributes {
        match attribute {
            enums::DocumentAttribute::DocumentAttributeSticker(_) => facts.kind = "sticker",
            enums::DocumentAttribute::DocumentAttributeVideo(video) => {
                facts.kind = "video";
                facts.width = i64::from(video.w);
                facts.height = i64::from(video.h);
                facts.duration_ms = seconds_to_ms(video.duration);
            }
            enums::DocumentAttribute::DocumentAttributeAudio(audio) => {
                facts.kind = if audio.voice || voice_flag {
                    "voice"
                } else {
                    "audio"
                };
                facts.duration_ms = i64::from(audio.duration) * 1000;
            }
            enums::DocumentAttribute::DocumentAttributeFilename(name) => {
                facts.file_name = Some(name.file_name.clone());
            }
            _ => {}
        }
    }
    facts
}

fn seconds_to_ms(seconds: f64) -> i64 {
    if seconds.is_finite() && seconds >= 0.0 {
        (seconds * 1000.0).round() as i64
    } else {
        0
    }
}

fn document_body(text: &str, document: Option<&enums::Document>) -> Body {
    let Some(enums::Document::Document(document)) = document else {
        return unsupported("an expired file");
    };
    if document.size > MAX_MEDIA_BYTES {
        return unsupported("a file this large");
    }
    let facts = document_facts(&document.attributes, false);
    let file_name = facts
        .file_name
        .unwrap_or_else(|| format!("file-{}", document.id));
    Body::Download(Download {
        kind: facts.kind.into(),
        caption: text.to_owned(),
        info: MediaInfo {
            mime: document.mime_type.clone(),
            file_name,
            size: usize::try_from(document.size).unwrap_or(0),
            width: facts.width,
            height: facts.height,
            duration_ms: facts.duration_ms,
            blurhash: String::new(),
        },
        location: enums::InputFileLocation::InputDocumentFileLocation(
            types::InputDocumentFileLocation {
                id: document.id,
                access_hash: document.access_hash,
                file_reference: document.file_reference.clone(),
                thumb_size: String::new(),
            },
        ),
        dc: document.dc_id,
    })
}

/// The reactions of a message, without senders (the server sends counts and our own choice).
pub fn reactions(summary: &types::MessageReactions) -> Vec<Reaction> {
    summary
        .results
        .iter()
        .filter_map(|count| {
            let enums::ReactionCount::ReactionCount(count) = count;
            let key = match count.reaction.as_ref() {
                enums::Reaction::ReactionEmoji(e) => e.emoticon.clone(),
                enums::Reaction::ReactionCustomEmoji(e) => custom_emoji_key(e.document_id),
                _ => return None,
            };
            Some(Reaction {
                key,
                count: i64::from(count.count),
                from_me: count.chosen_order.is_some(),
                senders: Vec::new(),
            })
        })
        .collect()
}

/// The reply target of a message, when it is a reply to a message (not a forum topic root).
pub fn reply_target(header: &enums::MessageReplyHeader) -> Option<ReplyTarget> {
    let enums::MessageReplyHeader::MessageReplyHeader(header) = header else {
        return None;
    };
    let message = header.reply_to_msg_id?;
    let topic = if header.forum_topic {
        header.reply_to_top_id.unwrap_or(message)
    } else {
        0
    };
    Some(ReplyTarget {
        peer: header.reply_to_peer_id.as_deref().map(peer_parts),
        message,
        topic,
    })
}

/// The forum topic a message belongs to (`0` outside topics).
pub fn topic_of(header: Option<&enums::MessageReplyHeader>) -> i32 {
    header
        .and_then(reply_target)
        .map_or(0, |target| target.topic)
}

/// The sentence Chirper shows for a service message. `who` is the actor, `name` resolves ids.
pub fn service_text(
    action: &enums::MessageAction,
    who: &str,
    name: impl Fn(i64) -> String,
) -> String {
    match action {
        enums::MessageAction::MessageActionChatCreate(a) => {
            format!("{who} created the group \"{}\"", a.title)
        }
        enums::MessageAction::MessageActionChatEditTitle(a) => {
            format!("{who} changed the group name to \"{}\"", a.title)
        }
        enums::MessageAction::MessageActionChatEditPhoto(_) => {
            format!("{who} changed the group photo")
        }
        enums::MessageAction::MessageActionChatDeletePhoto(_) => {
            format!("{who} removed the group photo")
        }
        enums::MessageAction::MessageActionChatAddUser(a) => {
            let added: Vec<String> = a.users.iter().map(|id| name(*id)).collect();
            format!("{who} added {}", added.join(", "))
        }
        enums::MessageAction::MessageActionChatDeleteUser(a) => {
            format!("{who} removed {}", name(a.user_id))
        }
        enums::MessageAction::MessageActionChatJoinedByLink(_) => {
            format!("{who} joined the group via an invite link")
        }
        enums::MessageAction::MessageActionChannelCreate(a) => {
            format!("The channel \"{}\" was created", a.title)
        }
        enums::MessageAction::MessageActionPinMessage(_) => format!("{who} pinned a message"),
        enums::MessageAction::MessageActionHistoryClear(_) => {
            format!("{who} cleared the history")
        }
        _ => "Service message".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_skip_entities_chirper_does_not_show() {
        let entities = vec![
            enums::MessageEntity::MessageEntityBold(types::MessageEntityBold {
                offset: 0,
                length: 2,
            }),
            enums::MessageEntity::MessageEntityMention(types::MessageEntityMention {
                offset: 3,
                length: 4,
            }),
        ];
        let found = spans(&entities);
        assert_eq!(found.len(), 1);
        assert_eq!(found.first().map(|s| s.tag.clone()), Some(Tag::Bold));
    }

    #[test]
    fn plain_text_without_entities_has_no_formatted_body() {
        match text_content("hello", &[]) {
            Content::Text { body, formatted } => {
                assert_eq!(body, "hello");
                assert_eq!(formatted, None);
            }
            other => panic!("unexpected content {other:?}"),
        }
    }

    #[test]
    fn document_attributes_pick_the_kind() {
        let voice = document_facts(
            &[enums::DocumentAttribute::DocumentAttributeAudio(
                types::DocumentAttributeAudio {
                    voice: true,
                    duration: 3,
                    title: None,
                    performer: None,
                    waveform: None,
                },
            )],
            false,
        );
        assert_eq!(voice.kind, "voice");
        assert_eq!(voice.duration_ms, 3000);
        let plain = document_facts(&[], false);
        assert_eq!(plain.kind, "file");
    }

    #[test]
    fn reactions_keep_custom_emoji_keys_and_own_choice() {
        let summary = types::MessageReactions {
            min: false,
            can_see_list: false,
            reactions_as_tags: false,
            results: vec![
                enums::ReactionCount::ReactionCount(types::ReactionCount {
                    chosen_order: Some(0),
                    reaction: Box::new(enums::Reaction::ReactionEmoji(types::ReactionEmoji {
                        emoticon: "👍".into(),
                    })),
                    count: 2,
                }),
                enums::ReactionCount::ReactionCount(types::ReactionCount {
                    chosen_order: None,
                    reaction: Box::new(enums::Reaction::ReactionCustomEmoji(
                        types::ReactionCustomEmoji { document_id: 77 },
                    )),
                    count: 1,
                }),
            ],
            recent_reactions: None,
            top_reactors: None,
        };
        let found = reactions(&summary);
        assert_eq!(found.len(), 2);
        assert_eq!(
            found.first().map(|r| (r.key.as_str(), r.from_me)),
            Some(("👍", true))
        );
        assert_eq!(found.get(1).map(|r| r.key.as_str()), Some("custom:77"));
    }
}
