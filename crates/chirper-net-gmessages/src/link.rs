//! The live channel to the phone: the `ReceiveMessages` long poll, the request/response pairing of
//! `SendMessage`, the acknowledgements and the token refresh. Port of `libgm/longpoll.go`,
//! `session_handler.go` and the token part of `client.go`.
//!
//! A [`Link`] runs one background task that reads the long poll and hands out what it reads:
//! responses go to the request waiting for them, pairing and update events go to a channel the
//! owner drains. Dropping the link stops the task.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use log::{debug, warn};
use prost::Message;
use tokio::sync::{mpsc, oneshot};

use chirper_bridge::{Error, Result};

use crate::api::{Api, config_version};
use crate::crypto::uuid_v4;
use crate::proto::pairing::{
    AuthMessage, Device, IncomingRpcMessage, OutgoingRpcData, RpcMessageData, RpcPairData, action,
    bugle_route, message_type,
};
use crate::wire::{self, LongPollItem, OutgoingRpc};

/// How long a request waits for the phone before it gives up.
pub const RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);
/// How often queued acknowledgements are flushed.
const ACK_INTERVAL: Duration = Duration::from_secs(5);
/// Refresh the token this long before it expires.
const TOKEN_REFRESH_BUFFER_MS: i64 = 60 * 60 * 1000;
/// Largest unparsed stream buffer the reader accepts.
const MAX_STREAM_BUFFER: usize = 16 << 20;
/// Largest block the reader keeps before it gives up on the stream.
const MAX_BLOCK: usize = 16 << 20;
/// The uncompressed-keyed "logged out" marker the phone sends on `GET_UPDATES`.
const LOGGED_OUT_MARKER: [u8; 2] = [0x72, 0x00];

/// One action to send to the phone, built with [`Call::new`] and the setters.
#[derive(Debug, Clone)]
pub struct Call<'a> {
    /// The `ActionType` number.
    pub action: i32,
    /// The binary protobuf parameters; empty for none.
    pub params: &'a [u8],
    /// Whether the parameters travel under the session keys.
    pub encrypt: bool,
    /// Time to live in microseconds; `None` for the session's default, `Some(0)` to omit it.
    pub ttl_us: Option<i64>,
    /// Whether the message belongs to the Google account pairing (`GAIA_2`).
    pub gaia: bool,
    /// The request id to use; a fresh one when `None`.
    pub request_id: Option<&'a str>,
    /// How long a [`Link::request`] waits for the answer.
    pub timeout: Duration,
}

impl<'a> Call<'a> {
    /// An encrypted action with no parameters and the default timeout.
    pub fn new(action: i32) -> Self {
        Self {
            action,
            params: &[],
            encrypt: true,
            ttl_us: None,
            gaia: false,
            request_id: None,
            timeout: RESPONSE_TIMEOUT,
        }
    }

    /// The parameters to send.
    pub fn params(mut self, params: &'a [u8]) -> Self {
        self.params = params;
        self
    }

    /// Send the parameters in clear, as the pairing messages do.
    pub fn clear(mut self) -> Self {
        self.encrypt = false;
        self
    }

    /// Set the time to live in microseconds (0 omits it).
    pub fn ttl(mut self, us: i64) -> Self {
        self.ttl_us = Some(us);
        self
    }

    /// Mark the message as part of the Google account pairing.
    pub fn gaia(mut self) -> Self {
        self.gaia = true;
        self
    }

    /// Use a specific request id.
    pub fn id(mut self, request_id: &'a str) -> Self {
        self.request_id = Some(request_id);
        self
    }

    /// Wait this long for the answer instead of the default.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// The answer to a request.
#[derive(Debug, Clone)]
pub struct Reply {
    /// The action the phone answered.
    pub action: i32,
    /// The decrypted body (binary protobuf of the response type).
    pub body: Vec<u8>,
}

/// What the link hands its owner, apart from the responses.
#[derive(Debug)]
pub enum Inbound {
    /// A pairing event from the phone.
    Pair(RpcPairData),
    /// An update event (`GET_UPDATES`), decrypted: the binary `UpdateEvents`.
    Update(Vec<u8>),
    /// The phone reports the browser logged out.
    LoggedOut,
    /// The long poll is connected and reading.
    Connected,
    /// The long poll dropped and is being re-opened.
    Disconnected(Error),
    /// The session can no longer be used; the owner must ask the user to log in again.
    Fatal(Error),
}

type Waiters = Arc<Mutex<HashMap<String, oneshot::Sender<Reply>>>>;

/// The shared state of one link.
#[derive(Clone)]
struct Shared {
    api: Api,
    waiters: Waiters,
    acks: Arc<Mutex<Vec<(String, Option<Device>)>>>,
    session_id: Arc<Mutex<String>>,
}

/// A live link to the phone. Owns the long poll task; dropping it stops the task.
pub struct Link {
    shared: Shared,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Link {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Link {
    /// Start the long poll. Unsolicited events are sent to `sink`.
    pub fn spawn(api: Api, sink: mpsc::UnboundedSender<Inbound>) -> Result<Self> {
        let session_id = uuid_v4()?;
        let shared = Shared {
            api,
            waiters: Arc::default(),
            acks: Arc::default(),
            session_id: Arc::new(Mutex::new(session_id)),
        };
        let task = tokio::spawn(run(shared.clone(), sink));
        Ok(Self { shared, task })
    }

    /// The API this link talks through.
    pub fn api(&self) -> &Api {
        &self.shared.api
    }

    /// Send an action and wait for the phone's answer.
    pub async fn request(&self, call: Call<'_>) -> Result<Reply> {
        let request_id = match call.request_id {
            Some(id) => id.to_owned(),
            None => uuid_v4()?,
        };
        let (tx, rx) = oneshot::channel();
        lock(&self.shared.waiters).insert(request_id.clone(), tx);
        if let Err(err) = self.send(&request_id, &call).await {
            lock(&self.shared.waiters).remove(&request_id);
            return Err(err);
        }
        match tokio::time::timeout(call.timeout, rx).await {
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(_)) => Err(Error::Network("the link closed before the phone answered".to_owned())),
            Err(_) => {
                lock(&self.shared.waiters).remove(&request_id);
                Err(Error::Network(
                    "the phone did not answer in time".to_owned(),
                ))
            }
        }
    }

    /// Send an action the phone does not answer (typing, read marks, pairing cancel).
    pub async fn send_only(&self, call: Call<'_>) -> Result<()> {
        let request_id = match call.request_id {
            Some(id) => id.to_owned(),
            None => uuid_v4()?,
        };
        self.send(&request_id, &call).await
    }

    async fn send(&self, request_id: &str, call: &Call<'_>) -> Result<()> {
        let session_id = lock(&self.shared.session_id).clone();
        let auth = self.shared.api.auth();
        let body = if call.params.is_empty() {
            Vec::new()
        } else if call.encrypt {
            let crypto = auth.request_crypto.as_ref().ok_or(Error::NotConnected)?;
            crypto.encrypt(call.params)?
        } else {
            call.params.to_vec()
        };
        let data = OutgoingRpcData {
            request_id: request_id.to_owned(),
            action: call.action,
            unencrypted_proto_data: if call.encrypt { Vec::new() } else { body.clone() },
            encrypted_proto_data: if call.encrypt { body } else { Vec::new() },
            session_id,
        };
        let message_data = data.encode_to_vec();
        let destinations: Vec<String> = Some(auth.dest_reg_id.clone())
            .filter(|id| !id.is_empty())
            .into_iter()
            .collect();
        let config = config_version();
        let rpc = OutgoingRpc {
            mobile: auth.mobile.as_ref(),
            request_id,
            bugle_route: bugle_route::DATA_EVENT,
            message_type: if call.gaia {
                message_type::GAIA_2
            } else {
                message_type::BUGLE_MESSAGE
            },
            message_data: &message_data,
            tachyon_auth_token: &auth.tachyon_token,
            config_version: &config,
            ttl: call.ttl_us.unwrap_or(auth.tachyon_ttl_us),
            dest_registration_ids: &destinations,
        };
        let path = wire::outgoing_rpc_to_pblite(&rpc);
        let url = self.shared.api.messaging_url("SendMessage");
        self.shared.api.post_pblite(&url, &path).await?;
        Ok(())
    }

    /// Queue an acknowledgement for a message the owner handled (used by tests).
    #[cfg(test)]
    fn queue_ack(&self, id: &str) {
        lock(&self.shared.acks).push((id.to_owned(), None));
    }
}

/// Send the queued acknowledgements in one `AckMessages` request. The stream loop calls it on a timer.
async fn flush_acks(shared: &Shared) -> Result<()> {
    let queued = std::mem::take(&mut *lock(&shared.acks));
    if queued.is_empty() {
        return Ok(());
    }
    let auth_msg = shared.api.auth_message(uuid_v4()?);
    let body = wire::ack_request(&auth_msg, &queued);
    let url = shared.api.messaging_url("AckMessages");
    match shared.api.post_pblite(&url, &body).await {
        Ok(_) => Ok(()),
        Err(err) => {
            // The acks are not lost: they go back on the queue for the next tick.
            let mut pending = lock(&shared.acks);
            if pending.len() + queued.len() <= 1024 {
                let mut restored = queued;
                restored.append(&mut pending);
                *pending = restored;
            }
            Err(err)
        }
    }
}

/// The reconnect loop: refresh the token, open the long poll, read it until it ends.
async fn run(shared: Shared, sink: mpsc::UnboundedSender<Inbound>) {
    let mut errors: u32 = 0;
    loop {
        if let Err(err) = ensure_token(&shared.api, false).await {
            let fatal = matches!(err, Error::Auth(_));
            if fatal {
                let _ = sink.send(Inbound::Fatal(err));
                return;
            }
            let _ = sink.send(Inbound::Disconnected(err));
            errors += 1;
            backoff(errors).await;
            continue;
        }
        let auth = shared.api.auth_message(uuid_v4().unwrap_or_default());
        let url = shared.api.messaging_url("ReceiveMessages");
        let request = wire::receive_messages_request(&auth);
        let response = match shared.api.open_stream(&url, &request).await {
            Ok(response) => response,
            Err(err @ Error::Auth(_)) => {
                let _ = sink.send(Inbound::Fatal(err));
                return;
            }
            Err(err) => {
                let _ = sink.send(Inbound::Disconnected(err));
                errors += 1;
                backoff(errors).await;
                continue;
            }
        };
        errors = 0;
        let _ = sink.send(Inbound::Connected);
        let outcome = read_stream(&shared, response, &sink).await;
        match outcome {
            Ok(()) => debug!("gmessages: long poll ended cleanly, reconnecting"),
            Err(err) => {
                warn!("gmessages: long poll stopped: {err}");
                let _ = sink.send(Inbound::Disconnected(err));
            }
        }
        backoff(1).await;
    }
}

async fn backoff(attempt: u32) {
    let seconds = (u64::from(attempt) + 1) * 5;
    tokio::time::sleep(Duration::from_secs(seconds.min(60))).await;
}

/// Read one long-poll response to its end, dispatching every block.
async fn read_stream(
    shared: &Shared,
    response: reqwest::Response,
    sink: &mpsc::UnboundedSender<Inbound>,
) -> Result<()> {
    let mut parser = StreamParser::default();
    let mut body = response.bytes_stream();
    let mut ack_tick = tokio::time::interval(ACK_INTERVAL);
    ack_tick.tick().await;
    loop {
        tokio::select! {
            chunk = body.next() => {
                let Some(chunk) = chunk else {
                    return Ok(());
                };
                let chunk = chunk.map_err(|_| Error::Network("the long poll was interrupted".to_owned()))?;
                for block in parser.push(&chunk)? {
                    match wire::long_poll_item(&block) {
                        Ok(LongPollItem::Data(message)) => {
                            handle_incoming(shared, *message, sink);
                        }
                        Ok(LongPollItem::Ack(count)) => debug!("gmessages: stream ack count {count}"),
                        Ok(LongPollItem::Heartbeat | LongPollItem::StartRead | LongPollItem::Unknown) => {}
                        Err(err) => debug!("gmessages: skipped a stream block: {err}"),
                    }
                }
            }
            _ = ack_tick.tick() => {
                if let Err(err) = flush_acks(shared).await {
                    debug!("gmessages: ack failed, will retry: {err}");
                }
            }
        }
    }
}

/// Route one incoming RPC: answer a waiting request, or hand the event to the owner.
fn handle_incoming(shared: &Shared, message: IncomingRpcMessage, sink: &mpsc::UnboundedSender<Inbound>) {
    lock(&shared.acks).push((message.response_id.clone(), message.browser.clone()));
    match message.bugle_route {
        bugle_route::PAIR_EVENT => match RpcPairData::decode(message.message_data.as_slice()) {
            Ok(pair) => {
                let _ = sink.send(Inbound::Pair(pair));
            }
            Err(_) => debug!("gmessages: a pairing event did not decode"),
        },
        bugle_route::DATA_EVENT => {
            let data = match RpcMessageData::decode(message.message_data.as_slice()) {
                Ok(data) => data,
                Err(_) => {
                    debug!("gmessages: a data event did not decode");
                    return;
                }
            };
            handle_data(shared, &data, sink);
        }
        _ => debug!("gmessages: an event on an unknown route"),
    }
}

fn handle_data(shared: &Shared, data: &RpcMessageData, sink: &mpsc::UnboundedSender<Inbound>) {
    let crypto = shared.api.auth().request_crypto;
    let is_encrypted = data.encrypted_data.is_some();
    let is_gaia_pairing = matches!(
        data.action,
        action::CREATE_GAIA_PAIRING_CLIENT_INIT | action::CREATE_GAIA_PAIRING_CLIENT_FINISHED
    );
    let body = match data.encrypted_data.as_deref() {
        Some(encrypted) => match crypto.as_ref().map(|c| c.decrypt(encrypted)) {
            Some(Ok(plain)) => plain,
            _ => {
                debug!("gmessages: an encrypted event could not be decrypted");
                return;
            }
        },
        None => data.unencrypted_data.clone(),
    };
    // A Google account receives stray unencrypted replies; only the pairing ones are answers.
    let ignore_stray = shared.api.auth().is_google_account()
        && !data.unencrypted_data.is_empty()
        && !is_encrypted
        && !is_gaia_pairing;
    if !ignore_stray {
        let waiter = lock(&shared.waiters).remove(&data.session_id);
        if let Some(waiter) = waiter {
            let _ = waiter.send(Reply {
                action: data.action,
                body,
            });
            return;
        }
    }
    if data.action == action::GET_UPDATES {
        if !is_encrypted && data.unencrypted_data == LOGGED_OUT_MARKER {
            let _ = sink.send(Inbound::LoggedOut);
            return;
        }
        let _ = sink.send(Inbound::Update(body));
    }
}

/// Refresh the Tachyon token when it is missing or close to expiry (or always, when `force`).
pub async fn ensure_token(api: &Api, force: bool) -> Result<()> {
    let auth = api.auth();
    if auth.browser.is_none() {
        return Ok(());
    }
    let now = now_ms();
    if !force && !auth.token_needs_refresh(now, TOKEN_REFRESH_BUFFER_MS) {
        return Ok(());
    }
    refresh_token(api, None).await
}

/// `RegisterRefresh`: sign a fresh request with the refresh key and store the new token.
pub async fn refresh_token(api: &Api, push: Option<&wire::PushRegistration>) -> Result<()> {
    let auth = api.auth();
    let (Some(browser), Some(key)) = (auth.browser.clone(), auth.refresh_key.clone()) else {
        return Err(Error::Auth("this session has no refresh key".to_owned()));
    };
    let request_id = uuid_v4()?;
    let timestamp_us = now_ms() * 1000;
    let signed = format!("{request_id}:{timestamp_us}");
    let signature = key.sign(signed.as_bytes())?;
    let message = api.auth_message(request_id);
    let body = wire::register_refresh_request_to_pblite(&message, &browser, timestamp_us, &signature, push);
    let url = api.instant_url(crate::api::REGISTRATION, "RegisterRefresh");
    let response = api.post_pblite(&url, &body).await?;
    let token = wire::register_refresh_response_from_pblite(&response)?
        .ok_or_else(|| Error::Protocol("the refresh answer has no token".to_owned()))?;
    if token.tachyon_auth_token.is_empty() {
        return Err(Error::Protocol("the refresh answer has no token".to_owned()));
    }
    api.update_auth(|auth| auth.set_token(token.tachyon_auth_token, token.ttl, now_ms()));
    Ok(())
}

/// Current Unix time in milliseconds.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Incremental reader of the long-poll stream: `[[`, then blocks separated by `,`, then `]]`.
#[derive(Debug, Default)]
pub struct StreamParser {
    started: bool,
    buffer: Vec<u8>,
}

impl StreamParser {
    /// Feed bytes; returns every block that is now complete, in order.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<serde_json::Value>> {
        self.buffer.extend_from_slice(chunk);
        if self.buffer.len() > MAX_STREAM_BUFFER {
            return Err(Error::Protocol("the long poll sent an oversized block".to_owned()));
        }
        if !self.started {
            if self.buffer.len() < 2 {
                return Ok(Vec::new());
            }
            if &self.buffer[..2] != b"[[" {
                return Err(Error::Protocol("the long poll did not open with [[".to_owned()));
            }
            self.buffer.drain(..2);
            self.started = true;
        }
        let mut blocks = Vec::new();
        loop {
            let start = self
                .buffer
                .iter()
                .position(|b| !b.is_ascii_whitespace() && *b != b',')
                .unwrap_or(self.buffer.len());
            self.buffer.drain(..start);
            if self.buffer.is_empty() || self.buffer.starts_with(b"]") && self.buffer.len() < 2 {
                break;
            }
            if self.buffer.starts_with(b"]]") {
                self.buffer.clear();
                break;
            }
            let Some(end) = json_value_end(&self.buffer) else {
                break;
            };
            let raw: Vec<u8> = self.buffer.drain(..end).collect();
            match serde_json::from_slice::<serde_json::Value>(&raw) {
                Ok(value) => blocks.push(value),
                Err(_) => debug!("gmessages: skipped a block that is not JSON"),
            }
            if raw.len() > MAX_BLOCK {
                return Err(Error::Protocol("the long poll sent an oversized block".to_owned()));
            }
        }
        Ok(blocks)
    }
}

/// The index just past the first complete JSON array or object in `bytes`, when there is one.
fn json_value_end(bytes: &[u8]) -> Option<usize> {
    let first = *bytes.first()?;
    if first != b'[' && first != b'{' {
        return None;
    }
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (index, byte) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'[' | b'{' => depth += 1,
            b']' | b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(index + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_yields_its_blocks_across_chunk_boundaries() {
        let stream = br#"[[[1,2],[null,"y"]]]"#;
        let mut parser = StreamParser::default();
        let mut blocks = Vec::new();
        for chunk in [&stream[..3], &stream[3..11], &stream[11..20], &stream[20..]] {
            blocks.extend(parser.push(chunk).expect("parses"));
        }
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0], serde_json::json!([1, 2]));
        assert_eq!(blocks[1], serde_json::json!([null, "y"]));
    }

    #[test]
    fn blocks_separated_by_commas_and_terminated_by_the_end_marker() {
        let mut parser = StreamParser::default();
        let blocks = parser
            .push(b"[[[1],\n[2],[3]]")
            .expect("parses");
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[2], serde_json::json!([3]));
        let tail = parser.push(b"]").expect("parses");
        assert!(tail.is_empty());
    }

    #[test]
    fn a_stream_that_does_not_open_with_double_bracket_is_a_protocol_error() {
        let mut parser = StreamParser::default();
        assert!(matches!(parser.push(b"<html>"), Err(Error::Protocol(_))));
    }

    #[test]
    fn brackets_inside_strings_do_not_end_a_block() {
        let mut parser = StreamParser::default();
        let stream = format!("[[{},{}]]", r#"["a]b\"]"]"#, r#"["c"]"#);
        let blocks = parser.push(stream.as_bytes()).expect("parses");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0], serde_json::json!(["a]b\"]"]));
    }

    #[test]
    fn the_scanner_finds_the_end_of_a_nested_value() {
        assert_eq!(json_value_end(b"[1,[2,[3]]]tail"), Some(11));
        assert_eq!(json_value_end(b"[1,[2"), None);
        assert_eq!(json_value_end(b"nope"), None);
    }

    #[tokio::test]
    async fn the_ack_queue_holds_what_it_is_given() {
        let api = Api::new(
            reqwest::Client::new(),
            crate::api::Hosts::under("http://127.0.0.1:1"),
            Arc::new(Mutex::new(crate::auth::AuthData::default())),
        );
        let (tx, _rx) = mpsc::unbounded_channel();
        let link = Link::spawn(api, tx).expect("spawns");
        link.queue_ack("a");
        link.queue_ack("b");
        assert_eq!(lock(&link.shared.acks).len(), 2);
        drop(link);
    }
}
