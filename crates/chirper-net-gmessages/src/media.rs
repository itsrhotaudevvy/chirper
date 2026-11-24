//! Attachments: the resumable upload of an outgoing file and the download of an incoming one.
//! Port of `libgm/media.go`. Every file is sealed with a fresh AES-256-GCM key; the key travels in
//! the message's `MediaContent`, so the phone can decrypt the upload.

use prost::Message;
use reqwest::Method;

use chirper_bridge::{Error, Result};

use crate::api::Api;
use crate::crypto::{GcmChunks, random_bytes};
use crate::proto::chat::{
    AttachmentInfo, DownloadAttachmentRequest, StartMediaUploadRequest, UploadMediaResponse,
};

/// Largest attachment the driver moves in either direction.
pub const MAX_MEDIA_BYTES: usize = 100 << 20;

/// A file that has been uploaded and can be sent as a `MediaContent`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Uploaded {
    /// The Google id of the file.
    pub media_id: String,
    /// The key that decrypts it.
    pub key: Vec<u8>,
}

/// The `MediaFormats` value for a MIME type (the subset `libgm/media.go` names). Unknown types are 0.
pub fn format_for_mime(mime: &str) -> i32 {
    match mime {
        "image/jpeg" => 1,
        "image/jpg" => 2,
        "image/png" => 3,
        "image/gif" => 4,
        "image/wbmp" => 5,
        "image/bmp" | "image/x-ms-bmp" => 6,
        "video/mp4" => 8,
        "video/3gpp2" => 9,
        "video/3gpp" => 10,
        "audio/mpeg" => 15,
        "audio/mp4" => 19,
        _ => 0,
    }
}

/// Upload a file: encrypt it, start the resumable upload, and finish it with the sealed bytes.
pub async fn upload(api: &Api, data: &[u8], mime: &str) -> Result<Uploaded> {
    if data.len() > MAX_MEDIA_BYTES {
        return Err(Error::Unsupported("sending a file this large"));
    }
    let key = random_bytes(32)?;
    let sealed = GcmChunks::new(&key)?.encrypt(data)?;
    let start = start_upload(api, sealed.len(), mime).await?;
    let answer = api
        .raw(
            Method::POST,
            &start.url,
            &[
                ("content-type", "application/x-www-form-urlencoded;charset=UTF-8".to_owned()),
                ("x-goog-upload-header-content-length", sealed.len().to_string()),
                ("x-goog-upload-command", "upload, finalize".to_owned()),
                ("x-goog-upload-offset", "0".to_owned()),
                ("x-goog-upload-header-content-type", mime.to_owned()),
            ],
            Some(sealed),
        )
        .await?;
    let body = decode_base64_if_needed(&answer.body);
    let response = UploadMediaResponse::decode(body.as_slice())
        .map_err(|_| Error::Protocol("the upload answer is not valid protobuf".to_owned()))?;
    let media = response
        .media
        .ok_or_else(|| Error::Protocol("the upload answer names no media".to_owned()))?;
    if media.media_id.is_empty() {
        return Err(Error::Protocol("the upload answer names no media".to_owned()));
    }
    Ok(Uploaded {
        media_id: media.media_id,
        key,
    })
}

struct StartedUpload {
    url: String,
}

async fn start_upload(api: &Api, size: usize, mime: &str) -> Result<StartedUpload> {
    let auth = api.auth();
    let request = StartMediaUploadRequest {
        attachment_type: 1,
        auth_data: Some(api.auth_message(crate::crypto::uuid_v4()?)),
        mobile: auth.mobile.clone(),
    };
    let body = crate::pblite::base64_encode(&request.encode_to_vec());
    let answer = api
        .raw(
            Method::POST,
            api.upload_url(),
            &[
                ("content-type", "application/x-www-form-urlencoded;charset=UTF-8".to_owned()),
                ("x-goog-upload-protocol", "resumable".to_owned()),
                ("x-goog-upload-command", "start".to_owned()),
                ("x-goog-upload-header-content-length", size.to_string()),
                ("x-goog-upload-header-content-type", mime.to_owned()),
            ],
            Some(body.into_bytes()),
        )
        .await?;
    let url = answer
        .headers
        .get("x-goog-upload-url")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .ok_or_else(|| Error::Protocol("the upload start names no upload URL".to_owned()))?;
    Ok(StartedUpload { url })
}

/// Download an attachment and open it with its key.
pub async fn download(api: &Api, media_id: &str, key: &[u8]) -> Result<Vec<u8>> {
    let metadata = DownloadAttachmentRequest {
        info: Some(AttachmentInfo {
            attachment_id: media_id.to_owned(),
            encrypted: true,
        }),
        auth_data: Some(api.auth_message(crate::crypto::uuid_v4()?)),
    };
    let encoded = crate::pblite::base64_encode(&metadata.encode_to_vec());
    let answer = api
        .raw(
            Method::GET,
            api.upload_url(),
            &[
                ("x-goog-download-metadata", encoded),
                ("accept", "*/*".to_owned()),
            ],
            None,
        )
        .await?;
    if answer.body.len() > MAX_MEDIA_BYTES + 1024 {
        return Err(Error::Protocol("the attachment is too large".to_owned()));
    }
    GcmChunks::new(key)?.decrypt(&answer.body)
}

/// The upload answer is base64 of the protobuf when it is plain base64 text, and the raw protobuf otherwise.
fn decode_base64_if_needed(body: &[u8]) -> Vec<u8> {
    let looks_base64 = !body.is_empty()
        && body.len() % 4 == 0
        && body
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'+' || *b == b'/' || *b == b'=');
    if looks_base64 {
        if let Ok(decoded) = crate::pblite::base64_decode(&String::from_utf8_lossy(body)) {
            return decoded;
        }
    }
    body.to_vec()
}

