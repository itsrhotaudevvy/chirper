//! Conversions between the bridge runtime's domain types and the wire model / store rows.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chirper_bridge::model as bridge;

use crate::content;
use crate::ids;
use crate::model;
use crate::store::{Media as StoredMedia, Room, Store};

/// The `Network` document of the wire contract.
pub fn wire_network(driver: &dyn chirper_bridge::Network) -> model::Network {
    let caps = driver.capabilities();
    model::Network {
        id: driver.id().to_owned(),
        name: driver.name().to_owned(),
        color: driver.color().to_owned(),
        beta: false,
        available: driver.unavailable().is_none(),
        reason: driver.unavailable().unwrap_or_default(),
        login_flows: driver
            .login_flows()
            .into_iter()
            .map(|flow| model::LoginFlow {
                id: flow.id,
                name: flow.name,
                description: flow.description,
            })
            .collect(),
        capabilities: model::Capabilities {
            edits: caps.edits,
            reactions: caps.reactions,
            replies: caps.replies,
            redactions: caps.redactions,
            typing: caps.typing,
            read_receipts: caps.read_receipts,
            stickers: caps.stickers,
            voice: caps.voice,
        },
    }
}

/// The `LoginState.step` document of a driver step.
pub fn wire_step(step: &bridge::LoginStep) -> model::LoginStep {
    use model::LoginStepType;
    match step {
        bridge::LoginStep::UserInput {
            step_id,
            instructions,
            fields,
        } => model::LoginStep {
            kind: LoginStepType::UserInput,
            step_id: step_id.clone(),
            instructions: instructions.clone(),
            user_input: Some(model::UserInputStep {
                fields: fields
                    .iter()
                    .map(|f| model::InputField {
                        id: f.id.clone(),
                        name: f.name.clone(),
                        kind: f.kind.clone(),
                        description: f.description.clone(),
                        options: f.options.clone(),
                        pattern: f.pattern.clone(),
                    })
                    .collect(),
            }),
            ..Default::default()
        },
        bridge::LoginStep::Cookies {
            step_id,
            instructions,
            url,
            user_agent,
            fields,
            extract_js,
            wait_for_url_pattern,
        } => model::LoginStep {
            kind: LoginStepType::Cookies,
            step_id: step_id.clone(),
            instructions: instructions.clone(),
            cookies: Some(model::CookiesStep {
                url: url.clone(),
                user_agent: user_agent.clone(),
                fields: fields
                    .iter()
                    .map(|f| model::CookieField {
                        id: f.id.clone(),
                        required: f.required,
                        sources: f
                            .sources
                            .iter()
                            .map(|s| model::CookieSource {
                                kind: s.kind.clone(),
                                name: s.name.clone(),
                                cookie_domain: s.cookie_domain.clone(),
                            })
                            .collect(),
                    })
                    .collect(),
                extract_js: extract_js.clone(),
                wait_for_url_pattern: (!wait_for_url_pattern.is_empty())
                    .then(|| wait_for_url_pattern.clone()),
            }),
            ..Default::default()
        },
        bridge::LoginStep::DisplayAndWait {
            step_id,
            instructions,
            kind,
            data,
            image_url,
        } => model::LoginStep {
            kind: LoginStepType::DisplayAndWait,
            step_id: step_id.clone(),
            instructions: instructions.clone(),
            display_and_wait: Some(model::DisplayStep {
                kind: match kind.as_str() {
                    "qr" => model::DisplayKind::Qr,
                    "emoji" => model::DisplayKind::Emoji,
                    "code" => model::DisplayKind::Code,
                    "nothing" => model::DisplayKind::Nothing,
                    _ => model::DisplayKind::Unknown,
                },
                data: data.clone(),
                image_url: image_url.clone(),
            }),
            ..Default::default()
        },
        bridge::LoginStep::Complete { login_id } => model::LoginStep {
            kind: LoginStepType::Complete,
            complete: Some(model::CompleteStep {
                login_id: login_id.clone(),
            }),
            ..Default::default()
        },
    }
}

/// The value stored in `event.content_json` for a driver message. Media is written to disk first.
pub fn content_json(content: &bridge::Content, media_dir: &Path, store: &Store) -> StoredContent {
    let media = match content {
        bridge::Content::Media {
            kind,
            caption,
            info,
            data,
            thumbnail,
        } => {
            let media = save_media(
                media_dir,
                store,
                info,
                data,
                thumbnail.as_ref().map(|t| t.as_slice()),
            );
            match media {
                Ok(Some(media)) => Some((kind.clone(), caption.clone(), media)),
                Ok(None) => None,
                Err(err) => {
                    log::warn!("cannot store incoming media: {err}");
                    None
                }
            }
        }
        _ => None,
    };
    if let Some((kind, caption, media)) = media {
        let stored = media_content_json(&kind, &caption, &media);
        return StoredContent {
            json: stored,
            kind: kind_of_media(&kind),
        };
    }
    match content {
        bridge::Content::Text { body, formatted } => StoredContent {
            json: content::text_content(body, formatted.as_deref()),
            kind: "text".to_owned(),
        },
        bridge::Content::Emote { body } => StoredContent {
            json: serde_json::json!({"v": 1, "kind": "emote", "body": body}).to_string(),
            kind: "emote".to_owned(),
        },
        bridge::Content::Notice { body } => StoredContent {
            json: serde_json::json!({"v": 1, "kind": "notice", "body": body}).to_string(),
            kind: "notice".to_owned(),
        },
        bridge::Content::Location { body } => StoredContent {
            json: serde_json::json!({"v": 1, "kind": "location", "body": body}).to_string(),
            kind: "location".to_owned(),
        },
        bridge::Content::Unsupported { body } => StoredContent {
            json: content::system_content(body),
            kind: "unsupported".to_owned(),
        },
        // A media message that could not be stored keeps its caption as text.
        bridge::Content::Media { caption, .. } => StoredContent {
            json: content::text_content(caption, None),
            kind: "text".to_owned(),
        },
    }
}

/// The stored JSON plus the display kind of a converted message.
pub struct StoredContent {
    pub json: String,
    /// `text`, `image`, … for the chat preview; `system` is never produced here.
    pub kind: String,
}

fn kind_of_media(kind: &str) -> String {
    match kind {
        "image" | "video" | "audio" | "voice" | "sticker" => kind.to_owned(),
        _ => "file".to_owned(),
    }
}

fn media_content_json(kind: &str, caption: &str, media: &model::Media) -> String {
    content::media_content(&kind_of_media(kind), caption, media)
}

/// Write an incoming attachment into the media directory and record it.
pub fn save_media(
    media_dir: &Path,
    store: &Store,
    info: &bridge::MediaInfo,
    data: &Arc<Vec<u8>>,
    thumbnail: Option<&[u8]>,
) -> std::io::Result<Option<model::Media>> {
    if data.is_empty() {
        return Ok(None);
    }
    crate::secure::create_private_dir(media_dir)?;
    let media_id = chirper_bridge::ids::random_hex(16);
    let extension = extension_for(&info.mime, &info.file_name);
    let path = media_dir.join(format!("{media_id}.{extension}"));
    crate::secure::write_private(&path, data.as_slice())?;
    let mut thumbnail_uri = String::new();
    if let Some(thumb) = thumbnail
        && !thumb.is_empty()
    {
        let thumb_path = media_dir.join(format!("{media_id}.thumb.jpg"));
        crate::secure::write_private(&thumb_path, thumb)?;
        thumbnail_uri = chirper_bridge::ids::media_uri(&format!("{media_id}.thumb"));
    }
    store
        .put_media(&StoredMedia {
            media_id: media_id.clone(),
            mime: info.mime.clone(),
            size: data.len() as i64,
            file_name: info.file_name.clone(),
            path: path.to_string_lossy().into_owned(),
            encrypted_key_json: None,
            created_ts: 0,
        })
        .map_err(std::io::Error::other)?;
    Ok(Some(model::Media {
        url: chirper_bridge::ids::media_uri(&media_id),
        mime: info.mime.clone(),
        size: data.len() as i64,
        width: info.width,
        height: info.height,
        duration_ms: info.duration_ms,
        file_name: info.file_name.clone(),
        thumbnail_url: thumbnail_uri,
        blurhash: info.blurhash.clone(),
    }))
}

fn extension_for(mime: &str, file_name: &str) -> String {
    if let Some((_, ext)) = file_name.rsplit_once('.') {
        let ext = ext.to_ascii_lowercase();
        if !ext.is_empty() && ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
            return ext;
        }
    }
    match mime {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "video/mp4" => "mp4",
        "video/quicktime" => "mov",
        "audio/mpeg" => "mp3",
        "audio/ogg" => "ogg",
        "audio/mp4" => "m4a",
        "application/pdf" => "pdf",
        _ => "bin",
    }
    .to_owned()
}

/// The room row of a portal: the chat's local id, its ownership and its parent, if any.
pub fn room_from_meta(
    network: &str,
    login_id: &str,
    chat: &chirper_bridge::ids::ChatId,
    meta: &bridge::ChatMeta,
) -> (
    Room,
    Option<(chirper_bridge::ids::ChatId, bridge::PortalRef)>,
) {
    let parent = meta.parent.as_ref().map(|parent| {
        (
            chirper_bridge::ids::chat_id(network, &parent.id, &parent.receiver),
            parent.clone(),
        )
    });
    let room = Room {
        room_id: chat.as_str().to_owned(),
        bridge_id: network.to_owned(),
        login_id: login_id.to_owned(),
        portal_id: portal_key(&meta.portal),
        remote_id: meta.portal.id.clone(),
        name: meta.name.clone(),
        topic: meta.topic.clone(),
        avatar_url: meta.avatar_url.clone(),
        is_dm: meta.is_dm,
        is_space: meta.is_space,
        dm_user_id: meta
            .dm_user
            .as_ref()
            .map(|user| ids::sender_id(network, &user.id).into_inner())
            .unwrap_or_default(),
        created_ts: crate::store::now_ms(),
        parent_id: parent
            .as_ref()
            .map(|(id, _)| id.as_str().to_owned())
            .unwrap_or_default(),
        parent_remote_id: parent
            .as_ref()
            .map(|(_, portal)| portal.id.clone())
            .unwrap_or_default(),
        ..Default::default()
    };
    (room, parent)
}

/// A stable, human-readable portal key: `network/portal` or `network/receiver/portal`.
pub fn portal_key(portal: &bridge::PortalRef) -> String {
    if portal.receiver.is_empty() {
        portal.id.clone()
    } else {
        format!("{}/{}", portal.receiver, portal.id)
    }
}

/// The portal a stored room belongs to, used to address the network.
pub fn portal_of(room: &Room) -> bridge::PortalRef {
    let (receiver, id) = match room.portal_id.split_once('/') {
        Some((receiver, id)) if !room.remote_id.is_empty() && id == room.remote_id => {
            (receiver.to_owned(), room.remote_id.clone())
        }
        _ => (
            String::new(),
            if room.remote_id.is_empty() {
                room.portal_id.clone()
            } else {
                room.remote_id.clone()
            },
        ),
    };
    bridge::PortalRef { id, receiver }
}

/// The media directory of a data directory.
pub fn media_dir(data_dir: &Path) -> PathBuf {
    crate::store::media_dir(data_dir)
}

#[cfg(test)]
mod media_tests {
    use super::*;

    #[test]
    fn saved_media_is_owner_only_from_creation() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("chirper-media-modes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("media");
        let store = Store::open_in_memory().unwrap();
        let info = bridge::MediaInfo {
            mime: "image/png".to_owned(),
            file_name: "a.png".to_owned(),
            size: 3,
            ..Default::default()
        };
        let data = Arc::new(vec![1u8, 2, 3]);
        let media = save_media(&dir, &store, &info, &data, Some(b"thumb"))
            .unwrap()
            .unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        let stored = store.media(media.url.rsplit('/').next().unwrap()).unwrap();
        assert_eq!(mode(Path::new(&stored.path)), 0o600);
        let thumb = dir.join(format!(
            "{}.thumb.jpg",
            media.url.rsplit('/').next().unwrap()
        ));
        assert_eq!(mode(&thumb), 0o600);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
