//! Outbound payloads of `createMessage`, and the builders that fill them for text, replies and
//! media. The field names and the omit-when-empty rules are the reference's `SendMessage` family.

use rand::Rng;
use rand::distributions::Alphanumeric;
use serde::Serialize;

use chirper_bridge::{Error, Result};

use super::urn::Urn;
use crate::ids;

/// Largest upload the driver sends. The reference caps attachments at the same size.
pub const MAX_UPLOAD_BYTES: usize = 20 * 1024 * 1024;

/// The body of `createMessage`.
#[derive(Clone, Debug, Serialize)]
pub struct SendPayload {
    pub message: SendMessage,
    #[serde(rename = "mailboxUrn")]
    pub mailbox_urn: String,
    #[serde(rename = "trackingId")]
    pub tracking_id: String,
    #[serde(rename = "dedupeByClientGeneratedToken")]
    pub dedupe: bool,
}

/// The message inside a [`SendPayload`].
#[derive(Clone, Debug, Serialize)]
pub struct SendMessage {
    pub body: TextBody,
    #[serde(rename = "renderContentUnions", skip_serializing_if = "Vec::is_empty")]
    pub renders: Vec<Render>,
    #[serde(rename = "conversationUrn")]
    pub conversation_urn: String,
    #[serde(rename = "originToken")]
    pub origin_token: String,
}

/// Plain text. Formatting is not sent, so the attribute list is omitted.
#[derive(Clone, Debug, Serialize)]
pub struct TextBody {
    pub text: String,
}

/// One render unit. Exactly one field is set.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Render {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioRender>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<FileRender>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video: Option<VideoRender>,
    #[serde(
        rename = "repliedMessageContent",
        skip_serializing_if = "Option::is_none"
    )]
    pub replied: Option<RepliedRender>,
}

/// A voice message asset.
#[derive(Clone, Debug, Serialize)]
pub struct AudioRender {
    #[serde(rename = "assetUrn")]
    pub asset: String,
    #[serde(rename = "byteSize")]
    pub size: usize,
}

/// A file asset.
#[derive(Clone, Debug, Serialize)]
pub struct FileRender {
    #[serde(rename = "assetUrn")]
    pub asset: String,
    #[serde(rename = "byteSize")]
    pub size: usize,
    #[serde(rename = "mediaType")]
    pub media_type: String,
    pub name: String,
}

/// A video asset with the single progressive stream the reference sends.
#[derive(Clone, Debug, Serialize)]
pub struct VideoRender {
    pub media: String,
    pub thumbnail: Thumbnail,
    #[serde(rename = "trackingId")]
    pub tracking_id: String,
    #[serde(rename = "progressiveStreams")]
    pub streams: Vec<ProgressiveStream>,
}

/// The thumbnail description of a video. LinkedIn accepts the empty one the reference sends.
#[derive(Clone, Debug, Serialize)]
pub struct Thumbnail {
    pub artifacts: Vec<Size>,
    #[serde(rename = "rootUrl")]
    pub root_url: String,
}

/// A pixel size.
#[derive(Clone, Debug, Serialize)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

/// One progressive download of a video.
#[derive(Clone, Debug, Serialize)]
pub struct ProgressiveStream {
    #[serde(rename = "bitRate")]
    pub bit_rate: u32,
    pub height: u32,
    #[serde(rename = "mediaType")]
    pub media_type: String,
    pub size: usize,
    pub width: u32,
    #[serde(rename = "streamingLocations")]
    pub locations: Vec<Location>,
}

/// A streaming location.
#[derive(Clone, Debug, Serialize)]
pub struct Location {
    pub url: String,
}

/// A reply: the quoted message as LinkedIn wants it.
#[derive(Clone, Debug, Serialize)]
pub struct RepliedRender {
    #[serde(rename = "originalSenderUrn")]
    pub sender: String,
    #[serde(rename = "originalSendAt")]
    pub sent_at: i64,
    #[serde(rename = "originalMessageUrn")]
    pub message: String,
    #[serde(rename = "messageBody")]
    pub body: TextBody,
}

/// The message a reply quotes, as the driver knows it.
#[derive(Clone, Debug, Default)]
pub struct QuotedMessage {
    /// The profile id of the quoted sender.
    pub sender_id: String,
    pub sent_at_ms: i64,
    /// The remote message id, `urn:li:msg_message:(...)`.
    pub message_id: String,
    pub text: String,
}

/// Builds the text payload of a message, with an optional reply.
pub fn text_payload(
    mailbox: &Urn,
    conversation: &str,
    text: &str,
    reply: Option<&QuotedMessage>,
    origin: &str,
) -> SendPayload {
    let renders = reply
        .map(|quoted| vec![replied(quoted)])
        .unwrap_or_default();
    payload(mailbox, conversation, text, renders, origin)
}

/// Builds the payload of an attachment message: the caption is the text, the asset is the render.
pub fn media_payload(
    mailbox: &Urn,
    conversation: &str,
    caption: &str,
    render: Render,
    reply: Option<&QuotedMessage>,
    origin: &str,
) -> SendPayload {
    let mut renders = vec![render];
    if let Some(quoted) = reply {
        renders.push(replied(quoted));
    }
    payload(mailbox, conversation, caption, renders, origin)
}

fn payload(
    mailbox: &Urn,
    conversation: &str,
    text: &str,
    renders: Vec<Render>,
    origin: &str,
) -> SendPayload {
    SendPayload {
        message: SendMessage {
            body: TextBody {
                text: text.to_owned(),
            },
            renders,
            conversation_urn: conversation.to_owned(),
            origin_token: origin.to_owned(),
        },
        mailbox_urn: mailbox.to_urn_string(),
        tracking_id: tracking_id(),
        dedupe: false,
    }
}

/// A reply render unit.
pub fn replied(quoted: &QuotedMessage) -> Render {
    Render {
        replied: Some(RepliedRender {
            sender: ids::participant_urn(&quoted.sender_id),
            sent_at: quoted.sent_at_ms,
            message: quoted.message_id.clone(),
            body: TextBody {
                text: quoted.text.clone(),
            },
        }),
        ..Render::default()
    }
}

/// Checks an attachment before it is uploaded and returns the upload type for it.
pub fn upload_type(kind: &str, mime: &str, size: usize) -> Result<&'static str> {
    if size > MAX_UPLOAD_BYTES {
        return Err(Error::Unsupported("attachments over 20 MiB"));
    }
    match kind {
        "image" => Ok("MESSAGING_PHOTO_ATTACHMENT"),
        "file" => Ok("MESSAGING_FILE_ATTACHMENT"),
        "video" => Ok("MESSAGING_VIDEO_ATTACHMENT"),
        "voice" | "audio" if mime == "audio/mp4" => Ok("VOICE_MESSAGE"),
        "voice" | "audio" => Err(Error::Unsupported("voice messages that are not audio/mp4")),
        "sticker" => Err(Error::Unsupported("stickers")),
        _ => Err(Error::Unsupported("this attachment type")),
    }
}

/// The render unit of an uploaded attachment. `kind` and `size` must have passed [`upload_type`].
pub fn attachment_render(kind: &str, asset: String, name: &str, mime: &str, size: usize) -> Render {
    match kind {
        "video" => Render {
            video: Some(VideoRender {
                media: asset.clone(),
                thumbnail: Thumbnail {
                    artifacts: vec![Size {
                        width: 0,
                        height: 0,
                    }],
                    root_url: String::new(),
                },
                tracking_id: asset,
                streams: vec![ProgressiveStream {
                    bit_rate: 0,
                    height: 0,
                    media_type: mime.to_owned(),
                    size,
                    width: 0,
                    locations: vec![Location {
                        url: format!("blob:https://www.linkedin.com/{}", super::uuid_v4()),
                    }],
                }],
            }),
            ..Render::default()
        },
        "voice" | "audio" => Render {
            audio: Some(AudioRender { asset, size }),
            ..Render::default()
        },
        _ => Render {
            file: Some(FileRender {
                asset,
                size,
                media_type: mime.to_owned(),
                name: name.to_owned(),
            }),
            ..Render::default()
        },
    }
}

/// The 16-character tracking id the reference generates per message.
fn tracking_id() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(16)
        .map(char::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mailbox() -> Urn {
        Urn::parse("urn:li:fsd_profile:ME")
    }

    #[test]
    fn a_text_payload_has_the_reference_field_names() {
        let payload = text_payload(
            &mailbox(),
            "urn:li:msg_conversation:(urn:li:fsd_profile:ME,THREAD)",
            "hello",
            None,
            "txn-1",
        );
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["mailboxUrn"], "urn:li:fsd_profile:ME");
        assert_eq!(json["dedupeByClientGeneratedToken"], false);
        assert_eq!(json["message"]["body"]["text"], "hello");
        assert_eq!(json["message"]["originToken"], "txn-1");
        assert_eq!(
            json["message"]["conversationUrn"],
            "urn:li:msg_conversation:(urn:li:fsd_profile:ME,THREAD)"
        );
        assert!(json["message"].get("renderContentUnions").is_none());
        assert_eq!(json["trackingId"].as_str().map(str::len), Some(16));
    }

    #[test]
    fn a_reply_quotes_the_sender_as_a_participant_urn() {
        let quoted = QuotedMessage {
            sender_id: "ACoAA9".to_owned(),
            sent_at_ms: 1_700_000_000_000,
            message_id: "urn:li:msg_message:(urn:li:fsd_profile:ACoAA9,M1)".to_owned(),
            text: "earlier".to_owned(),
        };
        let json = serde_json::to_value(replied(&quoted)).unwrap();
        let reply = &json["repliedMessageContent"];
        assert_eq!(
            reply["originalSenderUrn"],
            "urn:li:msg_messagingParticipant:urn:li:fsd_profile:ACoAA9"
        );
        assert_eq!(reply["originalSendAt"], 1_700_000_000_000i64);
        assert_eq!(reply["originalMessageUrn"], quoted.message_id);
        assert_eq!(reply["messageBody"]["text"], "earlier");
    }

    #[test]
    fn voice_must_be_mp4_and_files_are_capped() {
        assert!(matches!(
            upload_type("voice", "audio/ogg", 10),
            Err(Error::Unsupported(_))
        ));
        assert_eq!(
            upload_type("voice", "audio/mp4", 10).unwrap(),
            "VOICE_MESSAGE"
        );
        assert!(matches!(
            upload_type("file", "application/pdf", MAX_UPLOAD_BYTES + 1),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            upload_type("sticker", "image/webp", 1),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn a_file_render_names_its_upload_type() {
        assert_eq!(
            upload_type("image", "image/png", 4).unwrap(),
            "MESSAGING_PHOTO_ATTACHMENT"
        );
        let render = attachment_render("image", "urn:li:asset:1".into(), "p.png", "image/png", 4);
        let json = serde_json::to_value(&render).unwrap();
        assert_eq!(json["file"]["assetUrn"], "urn:li:asset:1");
        assert_eq!(json["file"]["mediaType"], "image/png");
        assert_eq!(json["file"]["name"], "p.png");
        assert!(json.get("video").is_none());
    }

    #[test]
    fn uuids_have_the_canonical_shape() {
        let id = super::super::uuid_v4();
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        assert!(parts[2].starts_with('4'));
    }
}
