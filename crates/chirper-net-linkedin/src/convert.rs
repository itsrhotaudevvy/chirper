//! LinkedIn's messages and conversations as the bridge model: chats, senders, the content of a
//! message (text, quoted text, attachments downloaded here), replies, edits and reactions.
//!
//! Reference: `connector/tomatrix.go`, `connector/chatinfo.go`, `connector/handlelinkedin.go` and
//! `connector/linkedinfmt`.

use std::sync::Arc;

use chirper_bridge::model::{
    ChatMeta, Content, MediaInfo, PortalRef, Reaction, RemoteMessage, RemoteUser,
};

use crate::api::Api;
use crate::api::model::{
    Conversation, Message, MessagingParticipant, ReactionSummary, RenderContent, VectorImage,
};
use crate::fmt;
use crate::ids;

/// The largest attachment the driver downloads. Bigger ones become `Content::Unsupported`.
pub const MAX_MEDIA_BYTES: usize = 100 * 1024 * 1024;

/// The bridge's view of one participant.
pub fn remote_user(participant: &MessagingParticipant) -> RemoteUser {
    let kind = &participant.participant_type;
    let (display_name, avatar_url) = if let Some(member) = &kind.member {
        (
            join_names(&member.first_name.text, &member.last_name.text),
            image_url(member.profile_picture.as_ref()),
        )
    } else if let Some(named) = kind
        .organization
        .as_ref()
        .or(kind.custom.as_ref())
        .or(kind.agent.as_ref())
    {
        (named.name.text.clone(), image_url(named.image.as_ref()))
    } else {
        (String::new(), String::new())
    };
    RemoteUser {
        id: ids::user_id(&participant.entity_urn),
        display_name,
        avatar_url,
        ..RemoteUser::default()
    }
}

fn join_names(first: &str, last: &str) -> String {
    [first, last]
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ")
}

fn image_url(image: Option<&VectorImage>) -> String {
    image.map(VectorImage::largest_url).unwrap_or_default()
}

/// A conversation as a chat. A one-to-one chat is named after the other member when it has no
/// title; a group is named after its other members.
pub fn chat_meta(conversation: &Conversation, me_id: &str) -> ChatMeta {
    let group = conversation.group_chat.unwrap_or(false);
    let others: Vec<RemoteUser> = conversation
        .participants
        .iter()
        .filter(|participant| ids::user_id(&participant.entity_urn) != me_id)
        .map(remote_user)
        .collect();
    let name = if conversation.title.is_empty() {
        others
            .iter()
            .map(|user| user.display_name.as_str())
            .filter(|name| !name.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        conversation.title.clone()
    };
    let dm_user = if group { None } else { others.first().cloned() };
    let avatar_url = dm_user
        .as_ref()
        .map(|user| user.avatar_url.clone())
        .unwrap_or_default();
    ChatMeta {
        portal: PortalRef {
            id: ids::portal_id(&conversation.entity_urn),
            receiver: ids::portal_receiver(group, me_id),
        },
        name,
        avatar_url,
        is_dm: !group,
        member_count: i64::try_from(conversation.participants.len()).unwrap_or(i64::MAX),
        dm_user,
        ..ChatMeta::default()
    }
}

/// The reactions of a message, as the full set the bridge stores.
pub fn reactions(summaries: &[ReactionSummary]) -> Vec<Reaction> {
    summaries
        .iter()
        .filter(|summary| summary.count > 0 && !summary.emoji.is_empty())
        .map(|summary| Reaction {
            key: summary.emoji.clone(),
            count: summary.count,
            from_me: summary.viewer_reacted,
            senders: Vec::new(),
        })
        .collect()
}

/// The portal of a message: its conversation URN, falling back to the embedded conversation.
pub fn portal_of(message: &Message) -> String {
    if message.conversation_urn.is_empty() {
        message.conversation.entity_urn.clone()
    } else {
        message.conversation_urn.clone()
    }
}

/// Converts one message into the bridge's messages. A message with several attachments becomes
/// several parts; a single attachment takes the text as its caption.
///
/// `group` says whether the conversation is a group chat, which decides the portal receiver.
pub async fn messages(
    api: &Api,
    message: &Message,
    group: bool,
    live: bool,
    me_id: &str,
) -> Vec<RemoteMessage> {
    let portal = PortalRef {
        id: ids::portal_id(&portal_of(message)),
        receiver: ids::portal_receiver(group, me_id),
    };
    let id = ids::message_id(&message.entity_urn);
    let sender = remote_user(&message.sender);
    let from_me = sender.id == me_id;
    let edited = (message.render_format == "EDITED").then(|| id.clone());
    let mut reply_to = None;
    let (mut body, mut html) = text_of(message);
    let mut media: Vec<(usize, Content)> = Vec::new();
    for (index, render) in message.render_content.iter().enumerate() {
        if let Some(replied) = &render.replied {
            reply_to = reply_to.or_else(|| Some(ids::message_id(&replied.original.entity_urn)));
        } else if let Some(forwarded) = &render.forwarded {
            append_quote(&mut body, &mut html, forwarded);
        } else if let Some(host) = &render.host_urn {
            append_feed_link(&mut body, &mut html, &host.kind, &host.host_urn);
        } else if let Some(content) = attachment(api, render).await {
            media.push((index, content));
        }
    }
    let mut contents: Vec<(String, Content)> = Vec::new();
    let has_text = !body.is_empty();
    if media.len() == 1 {
        if let Some((index, mut content)) = media.pop() {
            if let Content::Media { caption, .. } = &mut content {
                *caption = body.clone();
            }
            contents.push((part_name(index), content));
        }
    } else {
        if has_text {
            contents.push((
                String::new(),
                Content::Text {
                    body: body.clone(),
                    formatted: html.clone(),
                },
            ));
        }
        contents.extend(
            media
                .into_iter()
                .map(|(index, content)| (part_name(index), content)),
        );
    }
    if contents.is_empty() {
        contents.push((
            String::new(),
            Content::Unsupported {
                body: "A LinkedIn message the driver cannot show yet".to_owned(),
            },
        ));
    }
    let single = contents.len() == 1;
    contents
        .into_iter()
        .enumerate()
        .map(|(position, (part, content))| RemoteMessage {
            portal: portal.clone(),
            id: id.clone(),
            part: if single { String::new() } else { part },
            sender: sender.clone(),
            timestamp_ms: message.delivered_at,
            content,
            reply_to: if position == 0 {
                reply_to.clone()
            } else {
                None
            },
            edit_of: edited.clone(),
            live,
            from_me,
        })
        .collect()
}

fn part_name(index: usize) -> String {
    format!("part_{index}")
}

/// The text and its HTML, with the subject as bold first line when there is one.
fn text_of(message: &Message) -> (String, Option<String>) {
    let rendered = fmt::render(&message.body);
    let (mut body, mut html) = (rendered.body, rendered.html);
    if !message.subject.is_empty() {
        let subject_html = format!("<b>{}</b>", fmt::escape_text(&message.subject));
        let base = html.take().unwrap_or_else(|| fmt::escape_text(&body));
        html = Some(if body.is_empty() {
            subject_html
        } else {
            format!("{subject_html}<br><br>{base}")
        });
        body = if body.is_empty() {
            message.subject.clone()
        } else {
            format!("{}\n\n{body}", message.subject)
        };
    }
    (body, html)
}

/// Appends a quoted, forwarded message the way the reference quotes it.
fn append_quote(
    body: &mut String,
    html: &mut Option<String>,
    forwarded: &crate::api::model::ForwardedMessage,
) {
    let sender = remote_user(&forwarded.original_sender).display_name;
    let inner = fmt::render(&forwarded.body);
    let inner_html = inner.html.unwrap_or_else(|| fmt::escape_text(&inner.body));
    let footer = &forwarded.footer.text;
    let base = html.take().unwrap_or_else(|| fmt::escape_text(body));
    body.push_str(&format!("\n>{sender}:\n{}\n{footer}", inner.body));
    *html = Some(format!(
        "{base}<blockquote><p><b>{}:</b></p>{inner_html}<p>{}</p></blockquote>",
        fmt::escape_text(&sender),
        fmt::escape_text(footer)
    ));
}

/// Appends the link of a feed update a message shares.
fn append_feed_link(body: &mut String, html: &mut Option<String>, kind: &str, host_urn: &str) {
    if kind != "FEED_UPDATE" {
        return;
    }
    let Some(url) = ids::feed_update_url(host_urn) else {
        return;
    };
    let base = html.take().unwrap_or_else(|| fmt::escape_text(body));
    body.push(' ');
    body.push_str(&url);
    let escaped = fmt::escape_text(&url);
    *html = Some(format!("{base} <a href=\"{escaped}\">{escaped}</a>"));
}

/// The attachment of one render unit, downloaded. `None` for render units that are not media.
async fn attachment(api: &Api, render: &RenderContent) -> Option<Content> {
    if let Some(audio) = &render.audio {
        return Some(fetch(api, &audio.url, "voice", "voice_message", "").await);
    }
    if let Some(external) = &render.external_media {
        let name = if external.title.is_empty() {
            "image"
        } else {
            external.title.as_str()
        };
        return Some(fetch(api, &external.media.url, "image", name, "image/gif").await);
    }
    if let Some(file) = &render.file {
        let name = if file.name.is_empty() {
            "file"
        } else {
            file.name.as_str()
        };
        return Some(fetch(api, &file.url, "file", name, &file.media_type).await);
    }
    if let Some(image) = &render.vector_image {
        return Some(fetch(api, &image.largest_url(), "image", "image", "image/jpeg").await);
    }
    if let Some(video) = &render.video {
        let stream = video.streams.first();
        let location = stream.and_then(|stream| stream.locations.first());
        let url = location.map(|location| location.url.as_str()).unwrap_or("");
        let mime = stream
            .map(|stream| stream.media_type.as_str())
            .unwrap_or("video/mp4");
        return Some(fetch(api, url, "video", "video", mime).await);
    }
    None
}

/// Downloads one attachment. A failure degrades to an unsupported part, so one attachment
/// cannot hide the rest of the message.
async fn fetch(api: &Api, url: &str, kind: &str, name: &str, mime_hint: &str) -> Content {
    if url.is_empty() {
        return unsupported(format!("{name} has no download address"));
    }
    match api.download(url, MAX_MEDIA_BYTES).await {
        Ok(Some(download)) => {
            let mime = if download.mime.is_empty() {
                mime_hint.to_owned()
            } else {
                download.mime
            };
            let size = download.data.len();
            Content::Media {
                kind: kind.to_owned(),
                caption: String::new(),
                info: MediaInfo {
                    mime,
                    file_name: name.to_owned(),
                    size,
                    ..MediaInfo::default()
                },
                data: Arc::new(download.data),
                thumbnail: None,
            }
        }
        Ok(None) => unsupported(format!("{name} is larger than 100 MiB")),
        Err(error) => {
            log::debug!("linkedin: an attachment could not be downloaded: {error}");
            unsupported(format!("{name} could not be downloaded"))
        }
    }
}

fn unsupported(body: String) -> Content {
    Content::Unsupported { body }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::model::{AttributedText, Conversation, MemberInfo, ParticipantType};

    fn member(urn: &str, first: &str, last: &str) -> MessagingParticipant {
        MessagingParticipant {
            participant_type: ParticipantType {
                member: Some(MemberInfo {
                    first_name: AttributedText {
                        text: first.to_owned(),
                        ..AttributedText::default()
                    },
                    last_name: AttributedText {
                        text: last.to_owned(),
                        ..AttributedText::default()
                    },
                    ..MemberInfo::default()
                }),
                ..ParticipantType::default()
            },
            entity_urn: format!("urn:li:msg_messagingParticipant:urn:li:fsd_profile:{urn}"),
        }
    }

    #[test]
    fn a_member_is_named_by_first_and_last_name() {
        let user = remote_user(&member("ACoAA2", "Ada", "Lovelace"));
        assert_eq!(user.id, "ACoAA2");
        assert_eq!(user.display_name, "Ada Lovelace");
    }

    #[test]
    fn a_direct_chat_is_named_after_the_other_member() {
        let conversation = Conversation {
            entity_urn: "urn:li:msg_conversation:(urn:li:fsd_profile:ME,T1)".to_owned(),
            participants: vec![
                member("ME", "Me", "Self"),
                member("ACoAA2", "Ada", "Lovelace"),
            ],
            group_chat: Some(false),
            ..Conversation::default()
        };
        let meta = chat_meta(&conversation, "ME");
        assert_eq!(meta.name, "Ada Lovelace");
        assert!(meta.is_dm);
        assert_eq!(meta.portal.receiver, "ME");
        assert_eq!(meta.dm_user.map(|user| user.id), Some("ACoAA2".to_owned()));
        assert_eq!(meta.member_count, 2);
    }

    #[test]
    fn a_group_chat_has_no_receiver_and_no_dm_user() {
        let conversation = Conversation {
            entity_urn: "urn:li:msg_conversation:(urn:li:fsd_profile:ME,T2)".to_owned(),
            title: "Team".to_owned(),
            participants: vec![member("ME", "Me", "Self"), member("ACoAA2", "Ada", "L")],
            group_chat: Some(true),
            ..Conversation::default()
        };
        let meta = chat_meta(&conversation, "ME");
        assert_eq!(meta.name, "Team");
        assert!(!meta.is_dm);
        assert_eq!(meta.portal.receiver, "");
        assert!(meta.dm_user.is_none());
    }

    #[test]
    fn reactions_keep_counts_and_the_local_users_flag() {
        let reactions = reactions(&[
            ReactionSummary {
                count: 2,
                emoji: "👍".to_owned(),
                viewer_reacted: true,
            },
            ReactionSummary {
                count: 0,
                emoji: "❤".to_owned(),
                viewer_reacted: false,
            },
        ]);
        assert_eq!(reactions.len(), 1);
        assert_eq!(reactions[0].key, "👍");
        assert_eq!(reactions[0].count, 2);
        assert!(reactions[0].from_me);
    }

    #[test]
    fn subject_and_text_join_with_a_blank_line() {
        let message = Message {
            subject: "Plans".to_owned(),
            body: AttributedText {
                text: "Tomorrow".to_owned(),
                ..AttributedText::default()
            },
            ..Message::default()
        };
        let (body, html) = text_of(&message);
        assert_eq!(body, "Plans\n\nTomorrow");
        assert_eq!(html.as_deref(), Some("<b>Plans</b><br><br>Tomorrow"));
    }

    #[test]
    fn feed_links_are_appended_to_the_text() {
        let mut body = "look".to_owned();
        let mut html = None;
        append_feed_link(
            &mut body,
            &mut html,
            "FEED_UPDATE",
            "urn:li:fsd_update:(urn:li:activity:7,MESSAGING_RESHARE,EMPTY,DEFAULT,false)",
        );
        assert_eq!(
            body,
            "look https://www.linkedin.com/feed/update/urn:li:activity:7"
        );
        assert!(html.is_some_and(|value| value.contains("<a href=")));
    }

    #[test]
    fn a_quote_keeps_the_original_text_and_sender() {
        let forwarded = crate::api::model::ForwardedMessage {
            body: AttributedText {
                text: "original".to_owned(),
                ..AttributedText::default()
            },
            original_sender: member("ACoAA3", "Grace", "Hopper"),
            ..Default::default()
        };
        let mut body = "fwd".to_owned();
        let mut html = None;
        append_quote(&mut body, &mut html, &forwarded);
        assert!(body.contains(">Grace Hopper:\noriginal"));
        assert!(html.is_some_and(|value| value.starts_with("fwd<blockquote>")));
    }
}
