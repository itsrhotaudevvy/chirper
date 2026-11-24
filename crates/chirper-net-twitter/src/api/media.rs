//! The legacy media upload (INIT, APPEND, FINALIZE, STATUS) that direct messages use, and the
//! download of attachments. Reference: `twittermeow/media.go` (`UploadMedia`).

use std::time::Duration;

use md5::{Digest, Md5};
use rand::Rng;
use reqwest::Method;
use serde::Deserialize;

use chirper_bridge::{Error, Result};

use super::query::Query;
use super::{Api, Body, Opts};

/// The category of a direct-message image.
pub const CATEGORY_IMAGE: &str = "dm_image";
/// The category of a direct-message video.
pub const CATEGORY_VIDEO: &str = "dm_video";
/// The category of a direct-message GIF.
pub const CATEGORY_GIF: &str = "dm_gif";
/// Bytes sent per APPEND segment, as the reference sends them.
const SEGMENT_BYTES: usize = 6 * 1024 * 1024;
/// How many STATUS checks are made before the upload is given up.
const MAX_STATUS_CHECKS: u32 = 120;

/// A finished upload: the id a message refers to.
#[derive(Debug, Clone)]
pub struct Uploaded {
    pub media_id: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Init {
    media_id_string: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Processing {
    state: String,
    check_after_secs: u64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Finalized {
    media_id_string: String,
    processing_info: Option<Processing>,
}

/// The upload URL for one command and its parameters.
fn command_url(api: &Api, query: &Query) -> String {
    format!(
        "{}/i/media/upload.json?{}",
        api.bases().upload,
        query.encode()
    )
}

/// Upload a file for a direct message and wait until X has processed it.
pub async fn upload(api: &Api, media_type: &str, category: &str, data: &[u8]) -> Result<Uploaded> {
    let mut init = Query::new();
    init.push("command", "INIT")
        .push("total_bytes", data.len())
        .push("media_type", media_type)
        .push("media_category", category);
    let started: Init = api
        .call_json(
            Method::POST,
            &command_url(api, &init),
            Body::None,
            &Opts::default(),
        )
        .await?;
    if started.media_id_string.is_empty() {
        return Err(Error::Protocol("X gave no media id".to_owned()));
    }
    let media_id = started.media_id_string;

    for (index, segment) in data.chunks(SEGMENT_BYTES).enumerate() {
        let mut append = Query::new();
        append
            .push("command", "APPEND")
            .push("media_id", &media_id)
            .push("segment_index", index);
        let (content_type, bytes) = multipart_segment(segment);
        api.call(
            Method::POST,
            &command_url(api, &append),
            Body::Raw {
                content_type,
                bytes,
            },
            &Opts::default(),
        )
        .await?;
    }

    let mut finalize = Query::new();
    finalize
        .push("command", "FINALIZE")
        .push("media_id", &media_id);
    if category == CATEGORY_IMAGE {
        finalize.push("original_md5", md5_hex(data));
    }
    let mut result: Finalized = api
        .call_json(
            Method::POST,
            &command_url(api, &finalize),
            Body::None,
            &Opts::default(),
        )
        .await?;

    for _ in 0..MAX_STATUS_CHECKS {
        let Some(processing) = result.processing_info.take() else {
            break;
        };
        match processing.state.as_str() {
            "" | "succeeded" => break,
            "pending" | "in_progress" => {}
            _ => return Err(Error::Protocol("X could not process the media".to_owned())),
        }
        tokio::time::sleep(Duration::from_secs(
            processing.check_after_secs.clamp(1, 30),
        ))
        .await;
        let mut status = Query::new();
        status.push("command", "STATUS").push("media_id", &media_id);
        result = api
            .call_json(
                Method::GET,
                &command_url(api, &status),
                Body::None,
                &Opts::default(),
            )
            .await?;
    }
    let id = if result.media_id_string.is_empty() {
        media_id
    } else {
        result.media_id_string
    };
    Ok(Uploaded { media_id: id })
}

/// The multipart body of one APPEND segment: a single `media` part named `blob`.
fn multipart_segment(segment: &[u8]) -> (String, Vec<u8>) {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::thread_rng();
    let suffix: String = (0..16)
        .map(|_| char::from(ALPHABET[rng.gen_range(0..ALPHABET.len())]))
        .collect();
    let boundary = format!("----WebKitFormBoundary{suffix}");
    let mut bytes = Vec::with_capacity(segment.len() + 256);
    bytes.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"media\"; filename=\"blob\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    bytes.extend_from_slice(segment);
    bytes.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), bytes)
}

/// The lowercase hex MD5 of a file, which X checks on image uploads.
fn md5_hex(data: &[u8]) -> String {
    Md5::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_segment_is_a_single_media_part_with_its_boundary() {
        let (content_type, bytes) = multipart_segment(b"DATA");
        let boundary = content_type.trim_start_matches("multipart/form-data; boundary=");
        assert!(boundary.starts_with("----WebKitFormBoundary"));
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.starts_with(&format!("--{boundary}\r\n")));
        assert!(text.contains("name=\"media\"; filename=\"blob\""));
        assert!(text.contains("\r\n\r\nDATA\r\n"));
        assert!(text.ends_with(&format!("--{boundary}--\r\n")));
    }

    #[test]
    fn the_image_checksum_is_the_md5_of_the_file() {
        // RFC 1321 test vector.
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    }
}
