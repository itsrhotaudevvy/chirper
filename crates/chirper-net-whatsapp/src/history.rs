//! History sync: the account's old messages, imported from the phone's history blobs and served
//! back to the engine page by page.
//!
//! WhatsApp has no server-side paging, so the blobs are the only source. Each blob is downloaded,
//! inflated (zlib) and decoded as `waHistorySync.HistorySync`. Its messages are kept in the
//! secrets store, never as files, because they hold bodies. They are grouped per chat, sorted by
//! time and cut into pages of at most 50 encoded `WebMessageInfo`. `backfill` serves the most
//! recent remaining page first and deletes it once it is delivered. When a chat has no pages and we
//! have a message of it to anchor on, an on-demand history request goes to our primary phone.
//!
//! Layout of the secrets (`ctx` is the login's):
//!
//! * `history-index/<chat>`: one line per page, `<max timestamp> <page number>`
//! * `history/<chat>/<page>`: `u32` big-endian length, then the encoded `WebMessageInfo`, repeated
//! * `history-next/<chat>`: the next page number

use std::io::Read;

use chirper_bridge::model::{BackfillPage, Content, PortalRef, RemoteMessage};
use chirper_bridge::{Error, Event, Result};
use prost::Message as _;

use crate::binary::{Jid, Server};
use crate::convert::{self, Body, MediaSpec};
use crate::ids;
use crate::proto::e2e::{HistorySyncNotification, Message};
use crate::proto::history::{HistorySync, WebMessageInfo};
use crate::session::Client;
use crate::transport::Wire;

/// Messages per stored page.
pub const PAGE_SIZE: usize = 50;

/// The largest inflated history blob we accept.
const MAX_HISTORY: usize = 256 << 20;

fn index_secret(chat: &str) -> String {
    format!("history-index/{chat}")
}

fn page_secret(chat: &str, page: u64) -> String {
    format!("history/{chat}/{page}")
}

fn next_secret(chat: &str) -> String {
    format!("history-next/{chat}")
}

/// One stored page: its newest timestamp and its number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageRef {
    pub newest_ms: u64,
    pub number: u64,
}

/// Parses the page index of a chat.
pub fn parse_index(text: &str) -> Vec<PageRef> {
    text.lines()
        .filter_map(|line| {
            let (newest, number) = line.split_once(' ')?;
            Some(PageRef {
                newest_ms: newest.parse().ok()?,
                number: number.parse().ok()?,
            })
        })
        .collect()
}

/// Renders a page index.
pub fn render_index(pages: &[PageRef]) -> String {
    pages
        .iter()
        .map(|page| format!("{} {}\n", page.newest_ms, page.number))
        .collect()
}

/// Encodes messages as one page: length-prefixed `WebMessageInfo` records.
pub fn encode_page(messages: &[WebMessageInfo]) -> Vec<u8> {
    let mut out = Vec::new();
    for message in messages {
        let bytes = message.encode_to_vec();
        out.extend_from_slice(&u32::try_from(bytes.len()).unwrap_or(u32::MAX).to_be_bytes());
        out.extend_from_slice(&bytes);
    }
    out
}

/// Decodes a page. A record that does not decode is a protocol error for the whole page.
pub fn decode_page(bytes: &[u8]) -> Result<Vec<WebMessageInfo>> {
    let mut messages = Vec::new();
    let mut rest = bytes;
    while !rest.is_empty() {
        let (length, tail) = rest
            .split_first_chunk::<4>()
            .ok_or_else(|| Error::Protocol("history: a page is truncated".to_owned()))?;
        let length = usize::try_from(u32::from_be_bytes(*length))
            .map_err(|_| Error::Protocol("history: a page is truncated".to_owned()))?;
        let record = tail
            .get(..length)
            .ok_or_else(|| Error::Protocol("history: a page is truncated".to_owned()))?;
        messages.push(
            WebMessageInfo::decode(record)
                .map_err(|_| Error::Protocol("history: a message is malformed".to_owned()))?,
        );
        rest = &tail[length..];
    }
    Ok(messages)
}

/// Inflates and decodes a history blob.
pub fn decode_history(blob: &[u8]) -> Result<HistorySync> {
    let mut inflated = Vec::new();
    flate2::read::ZlibDecoder::new(blob)
        .take(MAX_HISTORY as u64 + 1)
        .read_to_end(&mut inflated)
        .map_err(|_| Error::Protocol("history: the blob does not inflate".to_owned()))?;
    if inflated.len() > MAX_HISTORY {
        return Err(Error::Unsupported("a history blob larger than 256 MiB"));
    }
    HistorySync::decode(inflated.as_slice())
        .map_err(|_| Error::Protocol("history: the blob is malformed".to_owned()))
}

/// The timestamp of a stored message in milliseconds.
fn timestamp_ms(message: &WebMessageInfo) -> u64 {
    message.message_timestamp.unwrap_or(0).saturating_mul(1000)
}

/// The chat a stored message belongs to.
fn chat_of(message: &WebMessageInfo) -> Option<Jid> {
    message.key.as_ref()?.remote_jid.as_deref()?.parse().ok()
}

impl<W: Wire> Client<W> {
    /// Downloads and imports a history blob. Failures are logged: history is best effort.
    pub(crate) async fn process_history_notification(
        &mut self,
        notification: &HistorySyncNotification,
    ) {
        let blob = match &notification.initial_hist_bootstrap_inline_payload {
            Some(inline) => inline.clone(),
            None => {
                let spec = MediaSpec {
                    kind: "file",
                    app_info: "History",
                    mms_type: "md-msg-hist",
                    direct_path: notification.direct_path.clone().unwrap_or_default(),
                    media_key: notification.media_key.clone().unwrap_or_default(),
                    file_enc_sha256: notification.file_enc_sha256.clone().unwrap_or_default(),
                    file_sha256: notification.file_sha256.clone().unwrap_or_default(),
                    size: notification.file_length.unwrap_or(0),
                    ..Default::default()
                };
                match self.download_media(&spec).await {
                    Ok(blob) => blob,
                    Err(error) => {
                        log::debug!("the history blob could not be downloaded: {error}");
                        return;
                    }
                }
            }
        };
        match decode_history(&blob) {
            Ok(history) => {
                if let Err(error) = self.import_history(&history) {
                    log::debug!("the history could not be stored: {error}");
                }
            }
            Err(error) => log::debug!("the history blob is unusable: {error}"),
        }
    }

    /// Stores a decoded blob: push names, chat names, and the messages as pages.
    pub(crate) fn import_history(&mut self, history: &HistorySync) -> Result<()> {
        for pushname in &history.pushnames {
            if let (Some(id), Some(name)) = (&pushname.id, &pushname.pushname) {
                self.names.insert(id.clone(), name.clone());
            }
        }
        let mut per_chat: std::collections::BTreeMap<String, Vec<WebMessageInfo>> =
            Default::default();
        for conversation in &history.conversations {
            let Some(chat_text) = conversation.id.clone() else {
                continue;
            };
            let Ok(chat) = chat_text.parse::<Jid>() else {
                continue;
            };
            let name = conversation
                .display_name
                .clone()
                .filter(|name| !name.is_empty())
                .or_else(|| conversation.name.clone())
                .unwrap_or_default();
            self.events
                .push_back(Event::Chat(chirper_bridge::model::ChatMeta {
                    portal: self.portal(&chat),
                    name,
                    is_dm: chat.server != Server::Group,
                    ..Default::default()
                }));
            for entry in &conversation.messages {
                if let Some(message) = &entry.message {
                    per_chat
                        .entry(chat_text.clone())
                        .or_default()
                        .push(message.clone());
                }
            }
        }
        for (chat, mut messages) in per_chat {
            messages.sort_by_key(timestamp_ms);
            for block in messages.chunks(PAGE_SIZE) {
                self.store_page(&chat, block)?;
            }
        }
        Ok(())
    }

    /// Stores one page of messages for `chat` and indexes it.
    fn store_page(&mut self, chat: &str, messages: &[WebMessageInfo]) -> Result<()> {
        let number = match self.ctx.get_secret(&next_secret(chat))? {
            Some(bytes) => String::from_utf8_lossy(&bytes).parse::<u64>().unwrap_or(0),
            None => 0,
        };
        self.ctx
            .set_secret(&page_secret(chat, number), &encode_page(messages))?;
        self.ctx
            .set_secret(&next_secret(chat), (number + 1).to_string().as_bytes())?;
        let newest_ms = messages.iter().map(timestamp_ms).max().unwrap_or(0);
        let mut pages = self.page_index(chat)?;
        pages.push(PageRef { newest_ms, number });
        self.ctx
            .set_secret(&index_secret(chat), render_index(&pages).as_bytes())
    }

    fn page_index(&self, chat: &str) -> Result<Vec<PageRef>> {
        Ok(match self.ctx.get_secret(&index_secret(chat))? {
            Some(bytes) => parse_index(&String::from_utf8_lossy(&bytes)),
            None => Vec::new(),
        })
    }

    /// Serves one page of history, newest first. Asks the phone for more when there is none.
    pub(crate) async fn backfill_page(
        &mut self,
        portal: &PortalRef,
        _cursor: Option<String>,
        _limit: usize,
    ) -> Result<BackfillPage> {
        let chat_key = portal.id.clone();
        let mut pages = self.page_index(&chat_key)?;
        if pages.is_empty() {
            self.request_on_demand_history(portal).await?;
            return Ok(BackfillPage::default());
        }
        pages.sort_by_key(|page| page.newest_ms);
        let page = pages
            .pop()
            .ok_or_else(|| Error::Internal("history index".to_owned()))?;
        let bytes = self
            .ctx
            .get_secret(&page_secret(&chat_key, page.number))?
            .ok_or_else(|| Error::Protocol("history: a page is missing".to_owned()))?;
        let stored = decode_page(&bytes)?;
        self.ctx
            .delete_secret(&page_secret(&chat_key, page.number))?;
        self.ctx
            .set_secret(&index_secret(&chat_key), render_index(&pages).as_bytes())?;

        let mut messages = Vec::with_capacity(stored.len());
        for record in &stored {
            if let Some(remote) = self.remote_of_stored(record).await {
                messages.push(remote);
            }
        }
        Ok(BackfillPage {
            messages,
            cursor: Some(page.number.to_string()),
            has_more: !pages.is_empty(),
        })
    }

    /// Converts a stored history message, downloading its media.
    async fn remote_of_stored(&mut self, record: &WebMessageInfo) -> Option<RemoteMessage> {
        let key = record.key.as_ref()?;
        let chat = chat_of(record)?;
        let id = key.id.clone()?;
        let from_me = key.from_me.unwrap_or(false);
        let sender: Jid = if from_me {
            Jid {
                device: 0,
                agent: 0,
                ..self.own.clone()
            }
        } else if chat.server == Server::Group {
            record.participant.as_deref()?.parse().ok()?
        } else {
            chat.clone()
        };
        let message = record.message.clone().unwrap_or_default();
        let (content, reply_to) = self.history_content(&message).await?;
        let info = crate::receive::MessageInfo {
            id: id.clone(),
            chat: chat.clone(),
            sender: sender.clone(),
            is_group: chat.server == Server::Group,
            is_from_me: from_me,
            timestamp_s: i64::try_from(record.message_timestamp.unwrap_or(0)).unwrap_or(0),
            push_name: record.push_name.clone().unwrap_or_default(),
        };
        Some(RemoteMessage {
            portal: self.portal(&chat),
            id: ids::message_id(&chat, &sender, &id),
            part: String::new(),
            sender: self.remote_user(&info),
            timestamp_ms: info.timestamp_s.saturating_mul(1000),
            content,
            reply_to: reply_to.and_then(|reply| convert::reply_message_id(&chat, &reply)),
            edit_of: None,
            live: false,
            from_me,
        })
    }

    async fn history_content(
        &mut self,
        message: &Message,
    ) -> Option<(Content, Option<convert::ReplyTo>)> {
        match convert::classify(message) {
            Body::Empty => None,
            Body::Text {
                body,
                formatted,
                reply_to,
            } => Some((Content::Text { body, formatted }, reply_to)),
            Body::Location(body) => Some((Content::Location { body }, None)),
            Body::Contact(body) | Body::Unsupported(body) => {
                Some((Content::Unsupported { body }, None))
            }
            Body::Media { spec, reply_to } => {
                let content = match self.download_media(&spec).await {
                    Ok(data) => crate::receive::media_content(&spec, data),
                    Err(_) => Content::Unsupported {
                        body: format!("{} could not be downloaded", spec.kind),
                    },
                };
                Some((content, reply_to))
            }
        }
    }

    /// Asks our primary phone for the history of a chat older than the last message we saw.
    async fn request_on_demand_history(&mut self, portal: &PortalRef) -> Result<()> {
        if !self.requested_history.insert(portal.id.clone()) {
            return Ok(());
        }
        let Some(anchor) = self
            .last_seen
            .get(&portal.id)
            .and_then(|id| ids::parse_message_id(id))
        else {
            return Ok(());
        };
        let own_user = Jid {
            device: 0,
            agent: 0,
            ..self.own.clone()
        };
        let request = crate::proto::e2e::history_sync_request(
            crate::proto::e2e::HistorySyncOnDemandRequest {
                chat_jid: Some(portal.id.clone()),
                oldest_msg_id: Some(anchor.id.clone()),
                oldest_msg_from_me: Some(anchor.sender.user == self.own.user),
                on_demand_msg_count: Some(i32::try_from(PAGE_SIZE).unwrap_or(50)),
                oldest_msg_timestamp_ms: Some(convert::now_ms()),
                support_inline_response: Some(false),
                mode: Some(0),
            },
        );
        let id = crate::session::generate_message_id(&mut self.rng);
        self.send_peer_message(&own_user, &id, &request).await
    }

    /// A peer message: encrypted for our own devices only, without the device-sent wrapper.
    async fn send_peer_message(&mut self, to: &Jid, id: &str, message: &Message) -> Result<()> {
        let plain = crate::crypto::pad_message(&message.encode_to_vec(), &mut self.rng);
        let devices = self.user_devices(std::slice::from_ref(to)).await?;
        let (participants, identity) = self
            .encrypt_for(&devices, |_| plain.clone(), None, "")
            .await?;
        let mut children = vec![crate::binary::Node::new("participants").children(participants)];
        if identity {
            children.push(self.device_identity()?);
        }
        let node = crate::binary::Node::new("message")
            .attr("id", id)
            .attr("type", "text")
            .attr("to", to.to_string())
            .attr("category", "peer")
            .children(children);
        self.send(&node).await?;
        self.await_message_ack(id).await
    }
}
