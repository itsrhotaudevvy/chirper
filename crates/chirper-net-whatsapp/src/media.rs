//! Media: the media-connection query, downloads with decryption, and encrypted uploads.
//!
//! The host, the auth token and the URL shapes follow whatsmeow's `mediaconn.go`, `download.go`
//! and `upload.go` (Mozilla Public License 2.0). Bytes are checked against the hashes the message
//! carries before they reach the bridge.

use std::time::Duration;

use chirper_bridge::{Error, Result};
use rand::RngCore;
use tokio::time::Instant;

use crate::binary::{Jid, Node};
use crate::convert::{MediaSpec, now_ms, reply_of};
use crate::crypto::{self, EncryptedMedia, MAX_MEDIA};
use crate::proto::e2e::{
    AudioMessage, ContextInfo, DocumentMessage, ImageMessage, Message, StickerMessage, VideoMessage,
};
use crate::session::{Client, SERVER_JID};
use crate::transport::Wire;

/// The media hosts and the auth token, valid for `ttl` seconds.
#[derive(Clone, Debug)]
pub struct MediaConn {
    pub auth: String,
    pub hosts: Vec<String>,
    pub fetched: Instant,
    pub ttl: u64,
}

impl MediaConn {
    fn is_fresh(&self) -> bool {
        Instant::now() < self.fetched + Duration::from_secs(self.ttl)
    }
}

/// The HKDF info string of a media type.
pub fn info_string(app: &str) -> String {
    format!("WhatsApp {app} Keys")
}

/// Percent-encodes everything but the URL unreserved characters.
pub fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The URL-safe base64 of `bytes`, with padding, as the media URLs carry it.
pub fn url_safe_base64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE.encode(bytes)
}

/// Reads the `media_conn` answer: the auth token, its lifetime and the hosts.
pub fn parse_media_conn(node: &Node, fetched: Instant) -> Result<MediaConn> {
    let conn = node
        .child("media_conn")
        .ok_or_else(|| Error::Protocol("media: no media_conn in the answer".to_owned()))?;
    let auth = conn
        .get_attr("auth")
        .ok_or_else(|| Error::Protocol("media: no auth token".to_owned()))?;
    let ttl = conn
        .get_attr("ttl")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let hosts: Vec<String> = conn
        .child_nodes()
        .iter()
        .filter(|child| child.tag == "host")
        .filter_map(|child| child.get_attr("hostname"))
        .collect();
    if hosts.is_empty() {
        return Err(Error::Protocol("media: no media hosts".to_owned()));
    }
    Ok(MediaConn {
        auth,
        hosts,
        fetched,
        ttl,
    })
}

/// The `mediatype` label of an upload, from the media kind.
fn upload_kind(kind: &str) -> (&'static str, &'static str) {
    match kind {
        "image" | "sticker" => ("Image", "image"),
        "video" => ("Video", "video"),
        "audio" | "voice" => ("Audio", "audio"),
        _ => ("Document", "document"),
    }
}

impl<W: Wire> Client<W> {
    /// The media connection, queried when there is none or it expired.
    pub(crate) async fn media_connection(&mut self) -> Result<MediaConn> {
        if let Some(conn) = &self.media_conn
            && conn.is_fresh()
        {
            return Ok(conn.clone());
        }
        let id = self.next_id();
        let request = Node::new("iq")
            .attr("to", SERVER_JID)
            .attr("type", "set")
            .attr("xmlns", "w:m")
            .attr("id", id)
            .children(vec![Node::new("media_conn")]);
        let reply = self.request(request).await?;
        let conn = parse_media_conn(&reply, Instant::now())?;
        self.media_conn = Some(conn.clone());
        Ok(conn)
    }

    /// The base of a media URL: the test server when one is set, else the host itself.
    fn media_url(&self, host: &str) -> String {
        match &self.media_base {
            Some(base) => base.clone(),
            None => format!("https://{host}"),
        }
    }

    /// Downloads and decrypts the media an incoming message describes. Tries each host in turn.
    pub(crate) async fn download_media(&mut self, spec: &MediaSpec) -> Result<Vec<u8>> {
        if usize::try_from(spec.size).map_or(true, |size| size > MAX_MEDIA) {
            return Err(Error::Unsupported("media larger than 100 MiB"));
        }
        if spec.direct_path.is_empty() || spec.media_key.is_empty() {
            return Err(Error::Protocol(
                "media: the message carries no download key".to_owned(),
            ));
        }
        let conn = self.media_connection().await?;
        let mut last = Error::Network("no media host answered".to_owned());
        for host in &conn.hosts {
            let url = format!(
                "{}{}&hash={}&mms-type={}&__wa-mms=",
                self.media_url(host),
                spec.direct_path,
                url_safe_base64(&spec.file_enc_sha256),
                spec.mms_type,
            );
            match get_bytes(&self.ctx.http, &url).await {
                Ok(data) => {
                    let info = info_string(spec.app_info);
                    return crypto::decrypt_media(
                        &data,
                        &spec.media_key,
                        &info,
                        &spec.file_sha256,
                        &spec.file_enc_sha256,
                    );
                }
                Err(error) => last = error,
            }
        }
        Err(last)
    }

    /// Encrypts `plaintext` and uploads it. Returns the URL and direct path the message carries.
    pub(crate) async fn upload_media(
        &mut self,
        kind: &str,
        plaintext: &[u8],
    ) -> Result<(EncryptedMedia, [u8; 32], String, String)> {
        if plaintext.len() > MAX_MEDIA {
            return Err(Error::Unsupported("media larger than 100 MiB"));
        }
        let mut media_key = [0u8; 32];
        self.rng.fill_bytes(&mut media_key);
        let (app, mms) = upload_kind(kind);
        let encrypted = crypto::encrypt_media(plaintext, &media_key, &info_string(app))?;
        let conn = self.media_connection().await?;
        let token = url_safe_base64(&encrypted.file_enc_sha256);
        let host = conn
            .hosts
            .first()
            .ok_or_else(|| Error::Protocol("media: no media hosts".to_owned()))?;
        let url = format!(
            "{}/mms/{mms}/{token}?auth={}&token={}",
            self.media_url(host),
            percent_encode(&conn.auth),
            percent_encode(&token),
        );
        let response = self
            .ctx
            .http
            .post(&url)
            .header("Origin", "https://web.whatsapp.com")
            .header("Referer", "https://web.whatsapp.com/")
            .body(encrypted.data.clone())
            .send()
            .await
            .map_err(|_| Error::Network("the media upload failed".to_owned()))?;
        if !response.status().is_success() {
            return Err(Error::Network(format!(
                "the media upload answered {}",
                response.status().as_u16()
            )));
        }
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|_| Error::Protocol("media: the upload answer is not JSON".to_owned()))?;
        let direct_path = body
            .get("direct_path")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                Error::Protocol("media: the upload answer has no direct path".to_owned())
            })?
            .to_owned();
        let url = body
            .get("url")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        Ok((encrypted, media_key, url, direct_path))
    }

    /// The message that sends an attachment of `kind`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn media_message(
        &mut self,
        chat: &Jid,
        data: &[u8],
        mime: &str,
        file_name: &str,
        caption: &str,
        kind: &str,
        reply_to: Option<&str>,
    ) -> Result<Message> {
        let _ = chat;
        let (encrypted, media_key, url, direct_path) = self.upload_media(kind, data).await?;
        let context = reply_to
            .and_then(crate::ids::parse_message_id)
            .map(|target| ContextInfo {
                stanza_id: Some(target.id),
                participant: Some(crate::ids::portal_id(&target.sender)),
                ..Default::default()
            });
        let common_key = Some(media_key.to_vec());
        let file_sha256 = Some(encrypted.file_sha256.to_vec());
        let file_enc_sha256 = Some(encrypted.file_enc_sha256.to_vec());
        let length = Some(data.len() as u64);
        let timestamp = Some(now_ms() / 1000);
        let mimetype = Some(mime.to_owned());
        let message = match kind {
            "image" => Message {
                image_message: Some(ImageMessage {
                    url: Some(url),
                    mimetype,
                    caption: Some(caption.to_owned()),
                    file_sha256,
                    file_length: length,
                    media_key: common_key,
                    file_enc_sha256,
                    direct_path: Some(direct_path),
                    media_key_timestamp: timestamp,
                    context_info: context,
                    ..Default::default()
                }),
                ..Default::default()
            },
            "video" => Message {
                video_message: Some(VideoMessage {
                    url: Some(url),
                    mimetype,
                    caption: Some(caption.to_owned()),
                    file_sha256,
                    file_length: length,
                    media_key: common_key,
                    file_enc_sha256,
                    direct_path: Some(direct_path),
                    media_key_timestamp: timestamp,
                    context_info: context,
                    ..Default::default()
                }),
                ..Default::default()
            },
            "audio" | "voice" => Message {
                audio_message: Some(AudioMessage {
                    url: Some(url),
                    mimetype,
                    file_sha256,
                    file_length: length,
                    ptt: Some(kind == "voice"),
                    media_key: common_key,
                    file_enc_sha256,
                    direct_path: Some(direct_path),
                    media_key_timestamp: timestamp,
                    context_info: context,
                    ..Default::default()
                }),
                ..Default::default()
            },
            "sticker" => Message {
                sticker_message: Some(StickerMessage {
                    url: Some(url),
                    mimetype,
                    file_sha256,
                    file_length: length,
                    media_key: common_key,
                    file_enc_sha256,
                    direct_path: Some(direct_path),
                    media_key_timestamp: timestamp,
                    context_info: context,
                    ..Default::default()
                }),
                ..Default::default()
            },
            _ => Message {
                document_message: Some(DocumentMessage {
                    url: Some(url),
                    mimetype,
                    title: Some(file_name.to_owned()),
                    file_name: Some(file_name.to_owned()),
                    caption: Some(caption.to_owned()),
                    file_sha256,
                    file_length: length,
                    media_key: common_key,
                    file_enc_sha256,
                    direct_path: Some(direct_path),
                    media_key_timestamp: timestamp,
                    context_info: context,
                    ..Default::default()
                }),
                ..Default::default()
            },
        };
        Ok(message)
    }
}

/// The reply context of a message, if it has one.
pub fn reply_context(context: Option<&ContextInfo>) -> Option<crate::convert::ReplyTo> {
    context.and_then(reply_of)
}

/// GETs `url` and returns the body, failing on a non-success status.
async fn get_bytes(http: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let response = http
        .get(url)
        .header("Origin", "https://web.whatsapp.com")
        .send()
        .await
        .map_err(|_| Error::Network("the media download failed".to_owned()))?;
    if !response.status().is_success() {
        return Err(Error::Network(format!(
            "the media download answered {}",
            response.status().as_u16()
        )));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| Error::Network("the media download was cut short".to_owned()))?;
    if bytes.len() > MAX_MEDIA + 64 {
        return Err(Error::Unsupported("media larger than 100 MiB"));
    }
    Ok(bytes.to_vec())
}
