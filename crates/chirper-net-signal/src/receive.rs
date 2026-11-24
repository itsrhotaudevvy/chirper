//! The receive loop: server requests, envelope decryption and the conversion of plaintext
//! `Content` into bridge events.
//!
//! Ported from signalmeow's `receiving.go`, `receiving_decrypt.go` and the connector's
//! `handlesignal.go` (AGPL-3.0). Envelopes are handled one at a time, in arrival order, because
//! the Double Ratchet is order-sensitive. Every envelope is acknowledged with 200 unless storage
//! failed, in which case the server is asked to redeliver it.

use std::sync::Arc;

use log::debug;
use prost::Message as _;
use rand::rngs::OsRng;

use chirper_bridge::model::{Content as BridgeContent, DeliveryStatus, RemoteEventRef};
use chirper_bridge::{Error, Event, PortalRef, Result};
use chirper_signal_protocol::message::{PreKeySignalMessage, SignalMessage};
use chirper_signal_protocol::sealed_sender;
use chirper_signal_protocol::{ProtocolAddress, cipher};

use crate::attach;
use crate::convert::{self, Media, Origin};
use crate::ids;
use crate::provision::proto;
use crate::proto::{
    AttachmentPointer, Content, DataMessage, Envelope, SyncMessage, TypingMessage, ReceiptMessage,
    envelope_type, receipt_type, typing_action, content,
};
use crate::session::Shared;
use crate::ws::{IncomingRequest, WsPump};

/// Attachment flag: a voice note.
const VOICE_FLAG: u32 = 1;

/// Runs until the socket ends: answers each server request, then ends the event stream.
pub async fn run(shared: Arc<Shared>, pump: WsPump) {
    let WsPump { mut incoming, mut task } = pump;
    let reason = loop {
        tokio::select! {
            request = incoming.recv() => match request {
                Some(request) => handle_request(&shared, request).await,
                None => break String::new(),
            },
            ended = &mut task => {
                break match ended {
                    Ok(Ok(())) => String::new(),
                    Ok(Err(err)) => err.to_string(),
                    Err(_) => "the socket task stopped".to_owned(),
                };
            }
        }
    };
    shared.emit(Event::State {
        state: chirper_bridge::SessionState::TransientDisconnect,
        error: reason,
        message: String::new(),
        profile: None,
    });
    shared.close_events();
}

/// Answers one server request.
async fn handle_request(shared: &Shared, request: IncomingRequest) {
    let status = match (request.verb.as_str(), request.path.as_str()) {
        ("PUT", "/api/v1/message") => match Envelope::decode(request.body.as_slice()) {
            Ok(envelope) => match handle_envelope(shared, &envelope).await {
                Ok(()) => 200,
                Err(Error::Internal(message)) => {
                    debug!("signal: envelope not stored: {message}");
                    500
                }
                Err(err) => {
                    debug!("signal: envelope dropped: {err}");
                    200
                }
            },
            Err(_) => 400,
        },
        _ => 200,
    };
    let _ = shared.ws.respond(request.id, status).await;
}

/// Decrypts one envelope and emits its events.
pub async fn handle_envelope(shared: &Shared, envelope: &Envelope) -> Result<()> {
    let kind = envelope.r#type.unwrap_or(envelope_type::UNKNOWN);
    let client_ts = envelope.client_timestamp.unwrap_or(0);
    let server_ts = envelope.server_timestamp.unwrap_or(0);
    let source = aci_text(envelope.source_service_id.as_deref(), envelope.source_service_id_binary.as_deref());
    let destination = envelope
        .destination_service_id
        .as_deref()
        .unwrap_or_default();
    if destination.starts_with(ids::PNI_SERVICE_PREFIX) {
        debug!("signal: envelope for a phone-number identity is not handled");
        return Ok(());
    }
    if kind == envelope_type::SERVER_DELIVERY_RECEIPT {
        if let Some(from) = source {
            shared.emit(Event::Delivery {
                portal: portal(&from),
                txn_id: ids::message_id(shared.me(), client_ts),
                status: DeliveryStatus::Delivered,
                error: String::new(),
            });
        }
        return Ok(());
    }
    let content = match kind {
        envelope_type::UNIDENTIFIED_SENDER => {
            let Some(bytes) = envelope.content.as_deref() else {
                return Err(Error::Protocol("a sealed envelope has no content".to_owned()));
            };
            let Some((sender, device, inner)) = open_sealed(shared, bytes, server_ts)? else {
                return Ok(());
            };
            if sender == shared.me() && device == shared.device_id() {
                return Ok(());
            }
            let address = ProtocolAddress::new(sender.clone(), u8_device(device)?);
            let plain = match inner {
                Inner::PreKey(bytes) => decrypt_prekey(shared, &address, &bytes)?,
                Inner::Whisper(bytes) => decrypt_whisper(shared, &address, &bytes)?,
                Inner::SenderKey(_) => {
                    debug!("signal: a group message needs its group, which is not loaded");
                    return Ok(());
                }
                Inner::Plain(_) => {
                    debug!("signal: a sealed plaintext carries only an error receipt");
                    return Ok(());
                }
            };
            (sender, device, plain)
        }
        envelope_type::PREKEY_MESSAGE | envelope_type::DOUBLE_RATCHET => {
            let Some(from) = source else {
                return Err(Error::Protocol("an envelope has no sender".to_owned()));
            };
            let device = envelope.source_device_id.unwrap_or(1);
            let address = ProtocolAddress::new(from.clone(), u8_device(device)?);
            let bytes = envelope.content.clone().unwrap_or_default();
            let plain = if kind == envelope_type::PREKEY_MESSAGE {
                decrypt_prekey(shared, &address, &bytes)?
            } else {
                decrypt_whisper(shared, &address, &bytes)?
            };
            (from, device, plain)
        }
        envelope_type::PLAINTEXT_CONTENT => {
            debug!("signal: a plaintext envelope carries only an error receipt");
            return Ok(());
        }
        _ => return Ok(()),
    };
    let (sender, device, plain) = content;
    let content = Content::decode(plain.as_slice())
        .map_err(|_| Error::Protocol("a decrypted message is not Content".to_owned()))?;
    handle_content(shared, &sender, &content, server_ts.max(client_ts)).await
}

/// The decrypted body of a sealed envelope, by its wrapped type.
enum Inner {
    PreKey(Vec<u8>),
    Whisper(Vec<u8>),
    SenderKey(Vec<u8>),
    Plain(Vec<u8>),
}

/// Opens a sealed-sender envelope and validates its certificate against the production roots.
/// Returns `None` for an envelope this driver does not handle.
fn open_sealed(shared: &Shared, bytes: &[u8], server_ts: u64) -> Result<Option<(String, u32, Inner)>> {
    let content = shared.with_store(|store| {
        let identity = store.identity_key_pair().map_err(proto)?;
        sealed_sender::decrypt_to_content(&identity, bytes).map_err(proto)
    })?;
    let valid = PRODUCTION_TRUST_ROOTS
        .iter()
        .any(|root| content.sender().validate(root, server_ts));
    if !valid {
        return Err(Error::Protocol("a sealed sender certificate does not validate".to_owned()));
    }
    let sender = content.sender().sender_uuid().to_owned();
    let device = content.sender().sender_device_id();
    let contents = content.contents().to_vec();
    let inner = match content.message_type() {
        chirper_signal_protocol::protocol::CiphertextMessageType::PreKey => Inner::PreKey(contents),
        chirper_signal_protocol::protocol::CiphertextMessageType::Whisper => Inner::Whisper(contents),
        chirper_signal_protocol::protocol::CiphertextMessageType::SenderKey => Inner::SenderKey(contents),
        chirper_signal_protocol::protocol::CiphertextMessageType::Plaintext => Inner::Plain(contents),
    };
    Ok(Some((sender, device, inner)))
}

/// Signal's production sealed-sender trust roots (`sendercertificate.go` in libsignalgo, AGPL-3.0).
pub const PRODUCTION_TRUST_ROOTS_B64: [&str; 2] = [
    "BXu6QIKVz5MA8gstzfOgRQGqyLqOwNKHL6INkv3IHWMF",
    "BUkY0I+9+oPgDCn4+Ac6Iu813yvqkDr/ga8DzLxFxuk6",
];

/// The decoded production roots, in the order above.
static PRODUCTION_TRUST_ROOTS: std::sync::LazyLock<Vec<chirper_signal_protocol::PublicKey>> =
    std::sync::LazyLock::new(|| {
        use base64::Engine;
        PRODUCTION_TRUST_ROOTS_B64
            .iter()
            .filter_map(|text| base64::engine::general_purpose::STANDARD.decode(text).ok())
            .filter_map(|bytes| chirper_signal_protocol::PublicKey::deserialize(&bytes).ok())
            .collect()
    });

fn decrypt_prekey(shared: &Shared, address: &ProtocolAddress, bytes: &[u8]) -> Result<Vec<u8>> {
    let message = PreKeySignalMessage::deserialize(bytes).map_err(proto)?;
    let plain = shared.with_store(|store| {
        cipher::decrypt_prekey_message(store, address, &message, &mut OsRng).map_err(proto)
    })?;
    strip_padding(&plain).map(<[u8]>::to_vec)
}

fn decrypt_whisper(shared: &Shared, address: &ProtocolAddress, bytes: &[u8]) -> Result<Vec<u8>> {
    let message = SignalMessage::deserialize(bytes).map_err(proto)?;
    let plain = shared.with_store(|store| {
        cipher::decrypt_signal_message(store, address, &message, &mut OsRng).map_err(proto)
    })?;
    strip_padding(&plain).map(<[u8]>::to_vec)
}

/// Removes the ISO 7816-4 padding: a `0x80` marker followed by zeros.
pub fn strip_padding(bytes: &[u8]) -> Result<&[u8]> {
    for index in (0..bytes.len()).rev() {
        match bytes.get(index) {
            Some(0x80) => return Ok(&bytes[..index]),
            Some(0) => continue,
            _ => break,
        }
    }
    Err(Error::Protocol("the message padding is invalid".to_owned()))
}

/// Converts one decrypted `Content` into events.
async fn handle_content(shared: &Shared, sender: &str, content: &Content, fallback_ts: u64) -> Result<()> {
    let me = shared.me().to_owned();
    match &content.content {
        Some(content::Content::DataMessage(data)) => {
            let origin = Origin {
                me: me.clone(),
                sender: sender.to_owned(),
                from_me: sender == me,
                fallback_ts,
            };
            data_message(shared, &origin, &portal(sender), data, None).await
        }
        Some(content::Content::EditMessage(edit)) => {
            let Some(data) = &edit.data_message else {
                return Ok(());
            };
            let origin = Origin {
                me: me.clone(),
                sender: sender.to_owned(),
                from_me: sender == me,
                fallback_ts,
            };
            data_message(shared, &origin, &portal(sender), data, edit.target_sent_timestamp).await
        }
        Some(content::Content::SyncMessage(sync)) => handle_sync(shared, sync, fallback_ts).await,
        Some(content::Content::ReceiptMessage(receipt)) => {
            receipt_events(shared, sender, receipt);
            Ok(())
        }
        Some(content::Content::TypingMessage(typing)) => {
            typing_event(shared, sender, typing);
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Events of a sync message: our own other-device sends and read positions.
async fn handle_sync(shared: &Shared, sync: &SyncMessage, fallback_ts: u64) -> Result<()> {
    let me = shared.me().to_owned();
    if let Some(content::sync_message::Content::Sent(sent)) = &sync.content {
        let destination = sent
            .destination_service_id
            .clone()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Error::Protocol("a sent transcript has no destination".to_owned()))?;
        let origin = Origin {
            me: me.clone(),
            sender: me.clone(),
            from_me: true,
            fallback_ts: sent.timestamp.unwrap_or(fallback_ts),
        };
        let target = portal(&destination);
        if let Some(data) = &sent.message {
            data_message(shared, &origin, &target, data, None).await?;
        } else if let Some(edit) = &sent.edit_message {
            if let Some(data) = &edit.data_message {
                data_message(shared, &origin, &target, data, edit.target_sent_timestamp).await?;
            }
        }
    }
    for read in &sync.read {
        let Some(sender) = aci_text(read.sender_aci.as_deref(), read.sender_aci_binary.as_deref()) else {
            continue;
        };
        shared.emit(Event::ReadReceipt {
            portal: portal(&sender),
            user: shared.me_user(),
            event: RemoteEventRef::new(ids::message_id(&sender, read.timestamp.unwrap_or(0))),
        });
    }
    Ok(())
}

/// Downloads the attachments and converts one data message (or edit) in `chat`.
async fn data_message(
    shared: &Shared,
    origin: &Origin,
    chat: &PortalRef,
    data: &DataMessage,
    edit_target: Option<u64>,
) -> Result<()> {
    let mut media: Vec<Media> = Vec::new();
    for pointer in &data.attachments {
        media.push(attachment(shared, pointer).await);
    }
    if let Some(sticker) = data.sticker.as_ref().and_then(|s| s.data.as_ref()) {
        media.push(attachment_as(shared, sticker, "sticker").await);
    }
    let events = {
        let mut book = shared
            .reactions
            .lock()
            .map_err(|_| Error::Internal("the reaction state lock was poisoned".to_owned()))?;
        convert::data_events(origin, chat, data, edit_target, media, &mut book)
    };
    for event in events {
        shared.emit(event);
    }
    Ok(())
}

async fn attachment(shared: &Shared, pointer: &AttachmentPointer) -> Media {
    let kind = if pointer.flags.unwrap_or(0) & VOICE_FLAG != 0 {
        "voice"
    } else {
        media_kind(pointer.content_type.as_deref().unwrap_or_default())
    };
    attachment_as(shared, pointer, kind).await
}

async fn attachment_as(shared: &Shared, pointer: &AttachmentPointer, kind: &str) -> Media {
    attach::media_for(&shared.http, &shared.endpoints, pointer, kind).await
}

/// The bridge's media kind for a MIME type.
pub fn media_kind(mime: &str) -> &'static str {
    if mime.starts_with("image/") {
        "image"
    } else if mime.starts_with("video/") {
        "video"
    } else if mime.starts_with("audio/") {
        "audio"
    } else {
        "file"
    }
}

fn receipt_events(shared: &Shared, sender: &str, receipt: &ReceiptMessage) {
    let kind = receipt.r#type.unwrap_or(receipt_type::DELIVERY);
    for timestamp in &receipt.timestamp {
        let remote = ids::message_id(shared.me(), *timestamp);
        if kind == receipt_type::DELIVERY {
            shared.emit(Event::Delivery {
                portal: portal(sender),
                txn_id: remote,
                status: DeliveryStatus::Delivered,
                error: String::new(),
            });
        } else {
            let status_user = convert::user(sender);
            shared.emit(Event::ReadReceipt {
                portal: portal(sender),
                user: status_user,
                event: RemoteEventRef::new(remote),
            });
        }
    }
}

fn typing_event(shared: &Shared, sender: &str, typing: &TypingMessage) {
    let started = typing.action.unwrap_or(typing_action::STARTED) == typing_action::STARTED;
    shared.emit(Event::Typing {
        portal: portal(sender),
        senders: if started { vec![convert::user(sender)] } else { Vec::new() },
    });
}

/// The DM portal of a service id: the id itself.
pub fn portal(service_id: &str) -> PortalRef {
    PortalRef {
        id: ids::dm_portal_id(service_id),
        receiver: String::new(),
    }
}

/// An ACI from its text or binary form, when either is present.
pub fn aci_text(text: Option<&str>, binary: Option<&[u8]>) -> Option<String> {
    if let Some(text) = text.filter(|t| !t.is_empty()) {
        return Some(text.to_owned());
    }
    binary.and_then(|b| ids::uuid_from_bytes(b).ok())
}

fn u8_device(device: u32) -> Result<u8> {
    u8::try_from(device).map_err(|_| Error::Protocol("a device id is out of range".to_owned()))
}
