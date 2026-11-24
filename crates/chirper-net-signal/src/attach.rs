//! Attachments: the encrypted blob format, padding, CDN download and the v4 upload.
//!
//! Ported from signalmeow's `attachments.go` (AGPL-3.0). A blob is `iv (16) | AES-256-CBC
//! ciphertext | HMAC-SHA256 (32)` over `iv | ciphertext`, keyed by a 64-byte key (AES key, then
//! MAC key). The pointer's `digest` is the SHA-256 of the whole blob. Plaintext is padded before
//! encryption to a size the server cannot use to tell files apart, and truncated to `size` after.

use rand::RngCore;
use rand::rngs::OsRng;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use chirper_bridge::{Error, Result};
use chirper_signal_protocol::cipher;

use crate::Endpoints;
use crate::convert::Media;
use crate::keys::ensure_ok;
use crate::proto::AttachmentPointer;
use crate::provision::{hmac_sha256, proto};
use crate::session::Shared;
use crate::proto::attachment_pointer::Identifier;

/// The largest attachment the driver will download or upload.
pub const MAX_ATTACHMENT_BYTES: usize = 100 * 1024 * 1024;
const IV_LEN: usize = 16;
const MAC_LEN: usize = 32;
/// The smallest padded size Signal uses.
const MIN_PADDED: usize = 541;

/// The padded length of a plaintext of `len` bytes, as the reference computes it.
pub fn padded_len(len: usize) -> usize {
    let len = len.max(1) as f64;
    let steps = (len.ln() / 1.05f64.ln()).ceil();
    let padded = 1.05f64.powf(steps).floor() as usize;
    padded.max(MIN_PADDED).max(len as usize)
}

/// Splits a 64-byte attachment key into its AES and MAC halves.
fn split_key(key: &[u8]) -> Result<([u8; 32], &[u8])> {
    if key.len() != 64 {
        return Err(Error::Protocol("an attachment key is not 64 bytes".to_owned()));
    }
    let aes: [u8; 32] = key
        .get(..32)
        .and_then(|k| k.try_into().ok())
        .ok_or_else(|| Error::Protocol("an attachment key is not 64 bytes".to_owned()))?;
    Ok((aes, &key[32..]))
}

/// Verifies, then decrypts an attachment blob, checks the digest and returns `size` bytes.
pub fn decrypt_blob(blob: &[u8], key: &[u8], digest: Option<&[u8]>, size: u32) -> Result<Vec<u8>> {
    if let Some(expected) = digest {
        if expected.len() != 32 || Sha256::digest(blob).as_slice() != expected {
            return Err(Error::Protocol("the attachment digest does not match".to_owned()));
        }
    }
    if blob.len() < IV_LEN + MAC_LEN {
        return Err(Error::Protocol("the attachment is too short".to_owned()));
    }
    let (aes_key, mac_key) = split_key(key)?;
    let (signed, mac) = blob.split_at(blob.len() - MAC_LEN);
    let expected = hmac_sha256(mac_key, &[signed]).map_err(|_| Error::Protocol("attachment MAC".to_owned()))?;
    if !bool::from(subtle::ConstantTimeEq::ct_eq(expected.as_slice(), mac)) {
        return Err(Error::Protocol("the attachment MAC does not match".to_owned()));
    }
    let iv: [u8; 16] = signed[..IV_LEN]
        .try_into()
        .map_err(|_| Error::Protocol("the attachment IV is missing".to_owned()))?;
    let plaintext = cipher::aes_cbc_decrypt(&aes_key, &iv, &signed[IV_LEN..])
        .map_err(|_| Error::Protocol("the attachment does not decrypt".to_owned()))?;
    let size = usize::try_from(size).unwrap_or(usize::MAX);
    if plaintext.len() < size {
        return Err(Error::Protocol("the attachment is shorter than its size".to_owned()));
    }
    let mut plaintext = plaintext;
    plaintext.truncate(size);
    Ok(plaintext)
}

/// Pads, encrypts and authenticates `plaintext` under `key` with the given IV. Returns the blob.
pub fn encrypt_blob(plaintext: &[u8], key: &[u8; 64], iv: [u8; 16]) -> Result<Vec<u8>> {
    let mut padded = plaintext.to_vec();
    padded.resize(padded_len(plaintext.len()), 0);
    let (aes_key, mac_key) = split_key(key)?;
    let ciphertext = cipher::aes_cbc_encrypt(&aes_key, &iv, &padded).map_err(proto)?;
    let mut blob = Vec::with_capacity(IV_LEN + ciphertext.len() + MAC_LEN);
    blob.extend_from_slice(&iv);
    blob.extend_from_slice(&ciphertext);
    let mac = hmac_sha256(mac_key, &[&blob])?;
    blob.extend_from_slice(&mac);
    Ok(blob)
}

/// The CDN base URL for a CDN number. The test override replaces every host.
pub fn cdn_base(endpoints: &Endpoints, cdn: u32) -> String {
    if let Some(base) = &endpoints.cdn {
        return base.clone();
    }
    match cdn {
        2 => "https://cdn2.signal.org".to_owned(),
        3 => "https://cdn3.signal.org".to_owned(),
        _ => "https://cdn.signal.org".to_owned(),
    }
}

/// Downloads and decrypts the attachment a pointer names.
pub async fn download(http: &reqwest::Client, endpoints: &Endpoints, pointer: &AttachmentPointer) -> Result<Vec<u8>> {
    let path = match &pointer.identifier {
        Some(Identifier::CdnKey(key)) => format!("/attachments/{key}"),
        Some(Identifier::CdnId(id)) => format!("/attachments/{id}"),
        None => return Err(Error::Protocol("an attachment has no identifier".to_owned())),
    };
    let size = pointer.size.unwrap_or(0);
    if usize::try_from(size).unwrap_or(usize::MAX) > MAX_ATTACHMENT_BYTES {
        return Err(Error::Unsupported("attachments over 100 MiB"));
    }
    let url = format!("{}{path}", cdn_base(endpoints, pointer.cdn_number.unwrap_or(0)));
    let response = http
        .get(&url)
        .send()
        .await
        .map_err(|err| Error::Network(format!("attachment download: {err}")))?;
    let status = response.status().as_u16();
    if status == 404 || status == 403 {
        return Err(Error::Protocol("the attachment is gone from the server".to_owned()));
    }
    if !(200..300).contains(&status) {
        return Err(Error::Network(format!("the CDN answered {status}")));
    }
    if response.content_length().is_some_and(|n| n as usize > MAX_ATTACHMENT_BYTES) {
        return Err(Error::Unsupported("attachments over 100 MiB"));
    }
    let blob = response
        .bytes()
        .await
        .map_err(|err| Error::Network(format!("attachment body: {err}")))?;
    if blob.len() > MAX_ATTACHMENT_BYTES {
        return Err(Error::Unsupported("attachments over 100 MiB"));
    }
    let key = pointer
        .key
        .as_deref()
        .ok_or_else(|| Error::Protocol("an attachment has no key".to_owned()))?;
    decrypt_blob(&blob, key, pointer.digest.as_deref(), size)
}

/// Downloads a pointer into a [`Media`] for the converter. A failure is kept as `data: None`.
pub async fn media_for(
    http: &reqwest::Client,
    endpoints: &Endpoints,
    pointer: &AttachmentPointer,
    kind: &str,
) -> Media {
    let data = download(http, endpoints, pointer).await.ok();
    Media {
        kind: kind.to_owned(),
        info: chirper_bridge::MediaInfo {
            mime: pointer.content_type.clone().unwrap_or_default(),
            file_name: pointer.file_name.clone().unwrap_or_default(),
            size: data.as_ref().map(Vec::len).unwrap_or(0),
            width: i64::from(pointer.width.unwrap_or(0)),
            height: i64::from(pointer.height.unwrap_or(0)),
            duration_ms: 0,
            blurhash: pointer.blur_hash.clone().unwrap_or_default(),
        },
        data,
    }
}

/// The upload slot the server allocates (`GET /v4/attachments/form/upload`).
#[derive(Debug, Deserialize)]
pub struct UploadSlot {
    pub cdn: u32,
    pub key: String,
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
    #[serde(rename = "signedUploadLocation")]
    pub signed_upload_location: String,
}

/// Encrypts and uploads `plaintext`, returning the pointer to send in a message.
pub async fn upload(shared: &Shared, plaintext: &[u8]) -> Result<AttachmentPointer> {
    if plaintext.len() > MAX_ATTACHMENT_BYTES {
        return Err(Error::Unsupported("attachments over 100 MiB"));
    }
    let mut key = [0u8; 64];
    let mut iv = [0u8; 16];
    OsRng.fill_bytes(&mut key);
    OsRng.fill_bytes(&mut iv);
    let blob = encrypt_blob(plaintext, &key, iv)?;
    let response = shared
        .ws
        .request(
            "GET",
            &format!("/v4/attachments/form/upload?uploadLength={}", blob.len()),
            None,
            Vec::new(),
        )
        .await?;
    ensure_ok(response.status)?;
    let slot: UploadSlot = serde_json::from_slice(response.body.as_deref().unwrap_or_default())
        .map_err(|_| Error::Protocol("the upload slot is not JSON".to_owned()))?;
    if slot.cdn == 3 {
        tus_upload(&shared.http, &slot, &blob).await?;
    } else {
        legacy_upload(shared, &slot, &blob).await?;
    }
    let digest = Sha256::digest(&blob).to_vec();
    Ok(AttachmentPointer {
        identifier: Some(Identifier::CdnKey(slot.key.clone())),
        content_type: None,
        key: Some(key.to_vec()),
        size: Some(u32::try_from(plaintext.len()).unwrap_or(u32::MAX)),
        digest: Some(digest),
        cdn_number: Some(slot.cdn),
        ..AttachmentPointer::default()
    })
}

/// The resumable (TUS) upload used by CDN 3.
async fn tus_upload(http: &reqwest::Client, slot: &UploadSlot, blob: &[u8]) -> Result<()> {
    let mut request = http
        .post(&slot.signed_upload_location)
        .header("Tus-Resumable", "1.0.0")
        .header("Upload-Length", blob.len().to_string())
        .header(
            "Upload-Metadata",
            format!("filename {}", base64_of(slot.key.as_bytes())),
        )
        .header("Content-Type", "application/offset+octet-stream");
    for (name, value) in &slot.headers {
        request = request.header(name.as_str(), value.as_str());
    }
    let response = request
        .body(blob.to_vec())
        .send()
        .await
        .map_err(|err| Error::Network(format!("attachment upload: {err}")))?;
    ensure_ok(Some(u32::from(response.status().as_u16())))
}

/// The two-step upload for CDN 2: allocate a location, then PUT the blob to it.
async fn legacy_upload(shared: &Shared, slot: &UploadSlot, blob: &[u8]) -> Result<()> {
    let auth = shared.account.basic_auth();
    let mut allocate = shared
        .http
        .post(&slot.signed_upload_location)
        .header("Content-Type", "application/octet-stream")
        .header("Authorization", auth.clone());
    for (name, value) in &slot.headers {
        allocate = allocate.header(name.as_str(), value.as_str());
    }
    let response = allocate
        .send()
        .await
        .map_err(|err| Error::Network(format!("attachment allocate: {err}")))?;
    ensure_ok(Some(u32::from(response.status().as_u16())))?;
    let location = response
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| Error::Protocol("the upload allocation has no location".to_owned()))?
        .to_owned();
    let response = shared
        .http
        .put(&location)
        .header("Content-Type", "application/octet-stream")
        .header("Authorization", auth)
        .body(blob.to_vec())
        .send()
        .await
        .map_err(|err| Error::Network(format!("attachment upload: {err}")))?;
    ensure_ok(Some(u32::from(response.status().as_u16())))
}

fn base64_of(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> [u8; 64] {
        let mut k = [0u8; 64];
        for (i, byte) in k.iter_mut().enumerate() {
            *byte = i as u8;
        }
        k
    }

    #[test]
    fn padding_matches_the_reference_steps() {
        assert_eq!(padded_len(0), 541);
        assert_eq!(padded_len(100), 541);
        assert!(padded_len(10_000) >= 10_000);
        // The padded size is a step of the 1.05 series, so it never shrinks as the input grows.
        let mut last = 0;
        for len in (1..50_000).step_by(997) {
            let padded = padded_len(len);
            assert!(padded >= len && padded >= last);
            last = padded;
        }
    }

    #[test]
    fn an_encrypted_blob_decrypts_to_the_original_bytes() {
        let plaintext = b"the attachment body".to_vec();
        let blob = encrypt_blob(&plaintext, &key(), [3; 16]).expect("encrypts");
        let digest = Sha256::digest(&blob).to_vec();
        let back = decrypt_blob(&blob, &key(), Some(&digest), plaintext.len() as u32).expect("decrypts");
        assert_eq!(back, plaintext);
    }

    #[test]
    fn a_tampered_blob_or_digest_is_rejected() {
        let blob = encrypt_blob(b"abc", &key(), [4; 16]).expect("encrypts");
        let digest = Sha256::digest(&blob).to_vec();
        let mut bad = blob.clone();
        bad[20] ^= 1;
        assert!(decrypt_blob(&bad, &key(), None, 3).is_err());
        let mut wrong_digest = digest.clone();
        wrong_digest[0] ^= 1;
        assert!(decrypt_blob(&blob, &key(), Some(&wrong_digest), 3).is_err());
        assert!(decrypt_blob(&blob[..10], &key(), None, 3).is_err());
    }

    #[test]
    fn a_wrong_size_is_rejected() {
        let blob = encrypt_blob(b"abc", &key(), [5; 16]).expect("encrypts");
        assert!(decrypt_blob(&blob, &key(), None, 100_000).is_err());
    }

    #[test]
    fn a_short_key_is_rejected() {
        let blob = encrypt_blob(b"abc", &key(), [6; 16]).expect("encrypts");
        assert!(decrypt_blob(&blob, &key()[..32], None, 3).is_err());
    }

    #[test]
    fn cdn_numbers_map_to_the_reference_hosts() {
        let production = Endpoints::production();
        assert_eq!(cdn_base(&production, 0), "https://cdn.signal.org");
        assert_eq!(cdn_base(&production, 2), "https://cdn2.signal.org");
        assert_eq!(cdn_base(&production, 3), "https://cdn3.signal.org");
        let local = Endpoints {
            cdn: Some("http://127.0.0.1:9".to_owned()),
            ..production
        };
        assert_eq!(cdn_base(&local, 3), "http://127.0.0.1:9");
    }
}
