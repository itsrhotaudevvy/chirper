//! Event content, stored as versioned, Matrix-free JSON.
//!
//! New rows are written in this crate's own format:
//!
//! ```json
//! {"v":1,"kind":"text","body":"hi","formattedBody":null}
//! {"v":1,"kind":"image","body":"caption","media":{"url":"mxc://chirper.local/…","mime":"image/png",…}}
//! {"v":1,"kind":"system","body":"Ada joined"}
//! ```
//!
//! Rows written by the Go core (Matrix event content: `msgtype`, `url`, `info`, state events) are
//! still read through [`describe`], so an existing install keeps its history, edits and
//! attachments. Nothing here depends on a Matrix type or a homeserver.

use serde::{Deserialize, Serialize};

use crate::model::Media;

/// The stored content document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    v: u32,
    kind: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    formatted_body: Option<String>,
    #[serde(default)]
    media: Option<Media>,
}

/// The display form of one event.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Described {
    /// `text`, `notice`, `emote`, `location`, `image`, `video`, `audio`, `voice`, `file`,
    /// `sticker`, `system` or `unsupported`.
    pub kind: String,
    pub body: String,
    pub formatted: String,
    pub media: Option<Media>,
    pub edited: bool,
}

impl Described {
    pub fn is_message(&self) -> bool {
        !matches!(self.kind.as_str(), "system" | "unsupported")
    }
}

/// Describe a stored event. `edit_json` is the `m.new_content`-equivalent of an edit, if any.
pub fn describe(event_kind: &str, content_json: &str, edit_json: Option<&str>) -> Described {
    let mut out = describe_one(event_kind, content_json);
    if let Some(edit) = edit_json {
        let edited = describe_one(event_kind, edit);
        if edited.body != out.body || edited.formatted != out.formatted || edited.media.is_some() {
            out.body = edited.body;
            out.formatted = edited.formatted;
            if edited.media.is_some() {
                out.media = edited.media;
            }
            out.edited = true;
        }
    }
    out
}

fn describe_one(event_kind: &str, content_json: &str) -> Described {
    let value: serde_json::Value = match serde_json::from_str(content_json) {
        Ok(value) => value,
        Err(_) => {
            return Described {
                kind: "unsupported".to_owned(),
                ..Default::default()
            };
        }
    };
    if value.get("v").is_some() {
        return from_stored(&value);
    }
    from_legacy(event_kind, &value)
}

fn from_stored(value: &serde_json::Value) -> Described {
    match serde_json::from_value::<Stored>(value.clone()) {
        Ok(stored) => Described {
            kind: stored.kind,
            body: stored.body,
            formatted: stored.formatted_body.unwrap_or_default(),
            media: stored.media,
            edited: false,
        },
        Err(_) => Described {
            kind: "unsupported".to_owned(),
            ..Default::default()
        },
    }
}

/// The content the Go core wrote: `m.room.message` with `msgtype`, plus state events.
fn from_legacy(event_kind: &str, value: &serde_json::Value) -> Described {
    let text = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned()
    };
    match event_kind {
        "m.sticker" => legacy_media("sticker", value),
        "m.room.message" => match text("msgtype").as_str() {
            "m.text" => legacy_text("text", value),
            "m.notice" => legacy_text("notice", value),
            "m.emote" => legacy_text("emote", value),
            "m.image" => legacy_media("image", value),
            "m.video" => legacy_media("video", value),
            "m.audio" => {
                if value.get("org.matrix.msc3245.voice").is_some() {
                    legacy_media("voice", value)
                } else {
                    legacy_media("audio", value)
                }
            }
            "m.file" => legacy_media("file", value),
            "m.location" => legacy_text("location", value),
            _ => Described {
                kind: "unsupported".to_owned(),
                ..Default::default()
            },
        },
        // State events became timeline entries with a human-readable sentence.
        "m.room.name" => system(match text("name") {
            name if name.is_empty() => "removed the group name".to_owned(),
            name => format!("changed the group name to {name}"),
        }),
        "m.room.topic" => system(match text("topic") {
            topic if topic.is_empty() => "removed the topic".to_owned(),
            topic => format!("changed the topic to {topic}"),
        }),
        "m.room.avatar" => system("changed the group photo".to_owned()),
        "m.room.member" => system(match text("membership").as_str() {
            "join" => "joined".to_owned(),
            "leave" => "left".to_owned(),
            "invite" => "was invited".to_owned(),
            "ban" => "was banned".to_owned(),
            _ => "changed membership".to_owned(),
        }),
        "m.room.create" => system("created the chat".to_owned()),
        _ => Described {
            kind: "system".to_owned(),
            body: "updated the chat".to_owned(),
            ..Default::default()
        },
    }
}

fn legacy_text(kind: &str, value: &serde_json::Value) -> Described {
    let text = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned()
    };
    Described {
        kind: kind.to_owned(),
        body: text("body"),
        formatted: text("formatted_body"),
        media: None,
        edited: false,
    }
}

fn legacy_media(kind: &str, value: &serde_json::Value) -> Described {
    let text = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned()
    };
    let info = value.get("info");
    let num = |key: &str| {
        info.and_then(|i| i.get(key))
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
    };
    let info_text = |key: &str| {
        info.and_then(|i| i.get(key))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned()
    };
    let url = info_text("url");
    let url = if url.is_empty() { text("url") } else { url };
    let file_name = match text("filename") {
        name if name.is_empty() && kind == "file" => text("body"),
        name => name,
    };
    Described {
        kind: kind.to_owned(),
        body: text("body"),
        formatted: String::new(),
        media: Some(Media {
            url,
            mime: info_text("mimetype"),
            size: num("size"),
            width: num("w"),
            height: num("h"),
            duration_ms: num("duration"),
            file_name,
            thumbnail_url: info_text("thumbnail_url"),
            blurhash: info_text("blurhash"),
        }),
        edited: false,
    }
}

fn system(body: String) -> Described {
    Described {
        kind: "system".to_owned(),
        body,
        ..Default::default()
    }
}

/// The chat list summary of a described event.
pub fn preview_text(described: &Described) -> String {
    let text = match described.kind.as_str() {
        "text" | "notice" | "emote" | "location" | "system" => described.body.clone(),
        "image" => "📷 Photo".to_owned(),
        "video" => "🎥 Video".to_owned(),
        "voice" => "🎤 Voice message".to_owned(),
        "audio" => "🎵 Audio".to_owned(),
        "file" => format!(
            "📎 {}",
            described
                .media
                .as_ref()
                .map(|m| m.file_name.clone())
                .unwrap_or_default()
        ),
        "sticker" => "Sticker".to_owned(),
        _ => String::new(),
    };
    let text = text.trim().to_owned();
    text.chars().take(140).collect()
}

/// Stored content of a text message.
pub fn text_content(body: &str, formatted: Option<&str>) -> String {
    serde_json::json!({
        "v": 1,
        "kind": "text",
        "body": body,
        "formattedBody": formatted,
    })
    .to_string()
}

/// Stored content of an attachment.
pub fn media_content(kind: &str, caption: &str, media: &Media) -> String {
    serde_json::json!({
        "v": 1,
        "kind": kind,
        "body": caption,
        "media": media,
    })
    .to_string()
}

/// Stored content of a system entry (a membership or metadata change).
pub fn system_content(body: &str) -> String {
    serde_json::json!({
        "v": 1,
        "kind": "system",
        "body": body,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_new_format() {
        let d = describe(
            "message",
            &text_content("hello", Some("<b>hello</b>")),
            None,
        );
        assert_eq!(d.kind, "text");
        assert_eq!(d.body, "hello");
        assert_eq!(d.formatted, "<b>hello</b>");
    }

    #[test]
    fn reads_rows_written_by_the_go_core() {
        let d = describe(
            "m.room.message",
            r#"{"msgtype":"m.text","body":"old","formatted_body":"<i>old</i>"}"#,
            None,
        );
        assert_eq!((d.kind.as_str(), d.body.as_str()), ("text", "old"));
        assert_eq!(d.formatted, "<i>old</i>");

        let img = describe(
            "m.room.message",
            r#"{"msgtype":"m.image","body":"cat","url":"mxc://chirper.local/a","filename":"cat.png","info":{"mimetype":"image/png","size":12,"w":3,"h":4}}"#,
            None,
        );
        assert_eq!(img.kind, "image");
        let media = img.media.unwrap();
        assert_eq!(media.url, "mxc://chirper.local/a");
        assert_eq!(media.mime, "image/png");
        assert_eq!((media.width, media.height), (3, 4));

        let member = describe("m.room.member", r#"{"membership":"leave"}"#, None);
        assert_eq!(member.kind, "system");
        assert_eq!(member.body, "left");
    }

    #[test]
    fn an_edit_replaces_the_body() {
        let original = text_content("typo", None);
        let edit = text_content("fixed", None);
        let d = describe("message", &original, Some(&edit));
        assert!(d.edited);
        assert_eq!(d.body, "fixed");
    }

    #[test]
    fn an_edit_written_by_the_go_core_is_applied_to_a_legacy_row() {
        let d = describe(
            "m.room.message",
            r#"{"msgtype":"m.text","body":"old"}"#,
            Some(r#"{"msgtype":"m.text","body":"new"}"#),
        );
        assert!(d.edited);
        assert_eq!(d.body, "new");
    }

    #[test]
    fn previews_are_short_and_kind_aware() {
        assert_eq!(
            preview_text(&describe("message", &text_content("hi", None), None)),
            "hi"
        );
        let long = "x".repeat(400);
        assert_eq!(
            preview_text(&describe("message", &text_content(&long, None), None))
                .chars()
                .count(),
            140
        );
    }

    #[test]
    fn garbage_content_does_not_panic() {
        let d = describe("m.room.message", "not json", None);
        assert_eq!(d.kind, "unsupported");
    }
}
