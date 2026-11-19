//! The encrypted MTProto 2.0 session.
//!
//! [`Core`] is the protocol state machine: it seals outgoing messages, opens and validates
//! incoming ones, and works out what each of them means (a reply, a salt change, a clock
//! correction, a resend, an update). It has no I/O, so it is tested directly. [`MtSession`] wraps
//! it in one task that owns the socket: a reader task feeds frames, the session task sends commands
//! and frames, so no read is ever cancelled half way through a frame.

use std::collections::BTreeMap;
use std::io::Read;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flate2::read::GzDecoder;
use tokio::sync::{mpsc, oneshot};
use zeroize::Zeroizing;

use super::authkey::AuthKey;
use super::crypto::{self, CLIENT_X, CryptoError, SERVER_X};
use super::transport::{Transport, TransportError, TransportReader, TransportWriter};
use crate::tl::{
    self, Cursor, Deserializable, DeserializeError, Identifiable, RemoteCall, Serializable,
    functions, types,
};

/// Most messages one container may hold (the schema's limit is 1024).
pub const MAX_CONTAINER_MESSAGES: usize = 1024;
/// Largest decompressed `gzip_packed` payload.
pub const MAX_DECOMPRESSED: usize = 16 * 1024 * 1024;
/// How long a request waits for its answer.
pub const INVOKE_TIMEOUT: Duration = Duration::from_secs(60);
/// Keep-alive interval, and the disconnect delay the server is told to use.
pub const KEEPALIVE: Duration = Duration::from_secs(60);
pub const DISCONNECT_DELAY: i32 = 75;

const CONTAINER_ID: u32 = 0x73f1_f8dc;
const GZIP_ID: u32 = 0x3072_cfa1;
const RPC_RESULT_ID: u32 = 0xf35c_6d01;
const RPC_ERROR_ID: u32 = 0x2144_ca19;
const FUTURE_SALTS_ID: u32 = 0xae50_0895;
const MSG_DETAILED_INFO_ID: u32 = 0x276d_3ec6;
const MSG_NEW_DETAILED_INFO_ID: u32 = 0x809d_b6df;
const MSGS_STATE_INFO_ID: u32 = 0x04de_b57d;

/// Why an invoked request failed.
#[derive(Debug)]
pub enum InvokeError {
    /// The transport failed.
    Transport(TransportError),
    /// A cryptographic check failed; the connection is dropped.
    Crypto(CryptoError),
    /// An object could not be read.
    Deserialize(DeserializeError),
    /// The server sent something the protocol does not allow.
    Protocol(&'static str),
    /// The server answered with an RPC error.
    Rpc { code: i32, message: String },
    /// The server asked us to wait (FLOOD_WAIT_X).
    RateLimited { seconds: u32 },
    /// The request belongs to another data centre (*_MIGRATE_X).
    Migrate { dc: i32 },
    /// The session ended before the request was answered.
    Closed,
    /// No answer arrived in time.
    Timeout,
}

impl std::fmt::Display for InvokeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "{e}"),
            Self::Crypto(e) => write!(f, "{e}"),
            Self::Deserialize(e) => write!(f, "{e}"),
            Self::Protocol(m) => write!(f, "protocol error: {m}"),
            Self::Rpc { code, message } => write!(f, "RPC error {code}: {message}"),
            Self::RateLimited { seconds } => write!(f, "rate limited for {seconds} s"),
            Self::Migrate { dc } => write!(f, "migrate to data centre {dc}"),
            Self::Closed => write!(f, "the session is closed"),
            Self::Timeout => write!(f, "no answer in time"),
        }
    }
}

impl std::error::Error for InvokeError {}

impl From<TransportError> for InvokeError {
    fn from(e: TransportError) -> Self {
        Self::Transport(e)
    }
}

impl From<CryptoError> for InvokeError {
    fn from(e: CryptoError) -> Self {
        Self::Crypto(e)
    }
}

impl From<DeserializeError> for InvokeError {
    fn from(e: DeserializeError) -> Self {
        Self::Deserialize(e)
    }
}

/// Something the server sent that is not an answer to a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateFrame {
    /// An update or other service object, unparsed: its constructor id and its serialised bytes.
    Raw { constructor: u32, body: Vec<u8> },
    /// The server started a new session: updates may have been missed and must be fetched.
    Gap,
}

type Reply = oneshot::Sender<Result<Vec<u8>, InvokeError>>;

/// What handling one incoming frame produced.
pub enum Event {
    /// A frame to send now.
    Send(Vec<u8>),
    /// The answer to a request, or its failure.
    Reply {
        reply: Option<Reply>,
        result: Result<Vec<u8>, InvokeError>,
    },
    /// Something for the application.
    Update(UpdateFrame),
}

struct Pending {
    body: Vec<u8>,
    reply: Option<Reply>,
    resends: u8,
}

/// Microseconds since the Unix epoch.
pub fn now_micros() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or(0)
}

/// A message id for `unix_micros`: seconds in the high 32 bits, a fraction of a second in the low
/// 32 bits. The low two bits are zero and the low word is never zero.
pub fn msg_id_at(unix_micros: u128) -> i64 {
    let secs = i64::try_from(unix_micros / 1_000_000).unwrap_or(i64::MAX >> 32);
    let frac = ((unix_micros % 1_000_000) << 32) / 1_000_000;
    let frac = (frac as i64) & !3;
    let frac = if frac == 0 { 4 } else { frac };
    (secs << 32) | frac
}

/// A message id for the current time corrected by `offset_secs`.
pub fn msg_id_now(offset_secs: i64) -> i64 {
    let micros = now_micros() as i128 + i128::from(offset_secs) * 1_000_000;
    msg_id_at(u128::try_from(micros).unwrap_or(0))
}

fn random_padding(len: usize) -> Result<Vec<u8>, CryptoError> {
    let mut out = vec![0u8; len];
    getrandom::fill(&mut out).map_err(|_| CryptoError::Random)?;
    Ok(out)
}

fn gunzip(data: &[u8]) -> Result<Vec<u8>, InvokeError> {
    let mut out = Vec::new();
    GzDecoder::new(data)
        .take(MAX_DECOMPRESSED as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|_| InvokeError::Protocol("bad gzip_packed"))?;
    if out.len() > MAX_DECOMPRESSED {
        return Err(InvokeError::Protocol("gzip_packed too large"));
    }
    Ok(out)
}

fn rpc_error_of(body: &[u8]) -> Result<InvokeError, InvokeError> {
    let error = <types::RpcError as Deserializable>::deserialize(&mut Cursor::new(body))?;
    let message = error.error_message;
    if let Some(seconds) = message
        .strip_prefix("FLOOD_WAIT_")
        .and_then(|s| s.parse().ok())
    {
        return Ok(InvokeError::RateLimited { seconds });
    }
    if let Some(dc) = message
        .rsplit_once("_MIGRATE_")
        .and_then(|(_, dc)| dc.parse().ok())
    {
        return Ok(InvokeError::Migrate { dc });
    }
    Ok(InvokeError::Rpc {
        code: error.error_code,
        message,
    })
}

/// The protocol state of one session. Not thread-safe: one task owns it.
pub struct Core {
    key: Zeroizing<[u8; 256]>,
    key_id: u64,
    session_id: i64,
    salt: i64,
    time_offset: i64,
    last_msg_id: i64,
    content_count: i32,
    pending: BTreeMap<i64, Pending>,
    acks: Vec<i64>,
}

impl Core {
    /// A new session on `auth_key`, with a random session id.
    pub fn new(auth_key: &AuthKey, salt: i64, time_offset: i64) -> Result<Self, InvokeError> {
        let session_id = i64::from_le_bytes(crypto::random_array::<8>()?);
        Ok(Self {
            key: Zeroizing::new(*auth_key.as_bytes()),
            key_id: auth_key.id(),
            session_id,
            salt,
            time_offset,
            last_msg_id: 0,
            content_count: 0,
            pending: BTreeMap::new(),
            acks: Vec::new(),
        })
    }

    fn next_msg_id(&mut self) -> i64 {
        let mut id = msg_id_now(self.time_offset);
        if id <= self.last_msg_id {
            id = self.last_msg_id + 4;
        }
        self.last_msg_id = id;
        id
    }

    /// Sequence number: content-related messages get an odd number and advance the counter.
    fn next_seq(&mut self, content: bool) -> i32 {
        if content {
            let seq = self.content_count.saturating_mul(2).saturating_add(1);
            self.content_count = self.content_count.saturating_add(1);
            seq
        } else {
            self.content_count.saturating_mul(2)
        }
    }

    /// Encrypt one message into a transport payload.
    fn seal(&self, msg_id: i64, seq: i32, body: &[u8]) -> Result<Vec<u8>, InvokeError> {
        let mut plain = Vec::with_capacity(32 + body.len() + 1024);
        plain.extend_from_slice(&self.salt.to_le_bytes());
        plain.extend_from_slice(&self.session_id.to_le_bytes());
        plain.extend_from_slice(&msg_id.to_le_bytes());
        plain.extend_from_slice(&seq.to_le_bytes());
        let len =
            i32::try_from(body.len()).map_err(|_| InvokeError::Protocol("message too large"))?;
        plain.extend_from_slice(&len.to_le_bytes());
        plain.extend_from_slice(body);
        let random = u32::from_le_bytes(crypto::random_array::<4>()?);
        let pad = crypto::padding_len(plain.len(), random);
        plain.extend(random_padding(pad)?);
        let (msg_key, cipher) = crypto::encrypt_message(&self.key, CLIENT_X, &plain)?;
        let mut frame = Vec::with_capacity(24 + cipher.len());
        frame.extend_from_slice(&self.key_id.to_le_bytes());
        frame.extend_from_slice(&msg_key);
        frame.extend_from_slice(&cipher);
        Ok(frame)
    }

    fn ack_body(ids: Vec<i64>) -> Vec<u8> {
        let mut out = Vec::new();
        types::MsgsAck { msg_ids: ids }.serialize(&mut out);
        out
    }

    /// Frames for a new request. Pending acknowledgements ride along in a container.
    pub fn request(
        &mut self,
        body: Vec<u8>,
        reply: Option<Reply>,
    ) -> Result<Vec<Vec<u8>>, InvokeError> {
        let msg_id = self.next_msg_id();
        let seq = self.next_seq(true);
        self.pending.insert(
            msg_id,
            Pending {
                body: body.clone(),
                reply,
                resends: 0,
            },
        );
        if self.acks.is_empty() {
            return Ok(vec![self.seal(msg_id, seq, &body)?]);
        }
        let ack_id = self.next_msg_id();
        let ack_seq = self.next_seq(false);
        let ack = Self::ack_body(std::mem::take(&mut self.acks));
        let mut container = Vec::new();
        container.extend_from_slice(&CONTAINER_ID.to_le_bytes());
        container.extend_from_slice(&2i32.to_le_bytes());
        for (id, s, b) in [(ack_id, ack_seq, &ack), (msg_id, seq, &body)] {
            container.extend_from_slice(&id.to_le_bytes());
            container.extend_from_slice(&s.to_le_bytes());
            container.extend_from_slice(&(b.len() as i32).to_le_bytes());
            container.extend_from_slice(b);
        }
        let container_id = self.next_msg_id();
        let container_seq = self.next_seq(false);
        Ok(vec![self.seal(container_id, container_seq, &container)?])
    }

    /// A keep-alive `ping_delay_disconnect`, which needs no answer to be useful.
    pub fn keepalive(&mut self) -> Result<Vec<u8>, InvokeError> {
        let ping_id = i64::from_le_bytes(crypto::random_array::<8>()?);
        let mut body = Vec::new();
        functions::PingDelayDisconnect {
            ping_id,
            disconnect_delay: DISCONNECT_DELAY,
        }
        .serialize(&mut body);
        let msg_id = self.next_msg_id();
        let seq = self.next_seq(true);
        self.pending.insert(
            msg_id,
            Pending {
                body: body.clone(),
                reply: None,
                resends: 0,
            },
        );
        self.seal(msg_id, seq, &body)
    }

    /// The acknowledgements still owed, as one frame, if there are any.
    pub fn flush_acks(&mut self) -> Result<Option<Vec<u8>>, InvokeError> {
        if self.acks.is_empty() {
            return Ok(None);
        }
        let ack = Self::ack_body(std::mem::take(&mut self.acks));
        let id = self.next_msg_id();
        let seq = self.next_seq(false);
        Ok(Some(self.seal(id, seq, &ack)?))
    }

    /// Validate and open one incoming frame, then work out what it means. Any error means the
    /// frame must be dropped and the connection closed.
    pub fn handle(&mut self, frame: &[u8]) -> Result<Vec<Event>, InvokeError> {
        let (msg_id, body) = self.open(frame)?;
        let mut events = Vec::new();
        self.dispatch(&body, msg_id, 0, &mut events)?;
        Ok(events)
    }

    fn open(&self, frame: &[u8]) -> Result<(i64, Vec<u8>), InvokeError> {
        let mut head = Cursor::new(frame);
        let key_id = head
            .take(8)
            .map_err(|_| InvokeError::Protocol("short frame"))?;
        if key_id != self.key_id.to_le_bytes() {
            return Err(InvokeError::Protocol("auth key id mismatch"));
        }
        let msg_key: [u8; 16] = {
            let raw = head
                .take(16)
                .map_err(|_| InvokeError::Protocol("short frame"))?;
            let mut out = [0u8; 16];
            out.copy_from_slice(raw);
            out
        };
        let cipher = head.take_rest();
        let plain = crypto::decrypt_message(&self.key, SERVER_X, &msg_key, cipher)?;
        let mut c = Cursor::new(&plain);
        let _salt = c
            .take(8)
            .map_err(|_| InvokeError::Protocol("short message"))?;
        let session = i64::from_le_bytes(
            c.take(8)
                .map_err(|_| InvokeError::Protocol("short message"))?
                .try_into()
                .map_err(|_| InvokeError::Protocol("short message"))?,
        );
        if session != self.session_id {
            return Err(InvokeError::Protocol("session id mismatch"));
        }
        let msg_id = i64::from_le_bytes(
            c.take(8)
                .map_err(|_| InvokeError::Protocol("short message"))?
                .try_into()
                .map_err(|_| InvokeError::Protocol("short message"))?,
        );
        if msg_id & 1 == 0 {
            return Err(InvokeError::Protocol("server message id is not odd"));
        }
        let _seq = c
            .read_i32()
            .map_err(|_| InvokeError::Protocol("short message"))?;
        let len = c
            .read_i32()
            .map_err(|_| InvokeError::Protocol("short message"))?;
        let len = usize::try_from(len).map_err(|_| InvokeError::Protocol("negative length"))?;
        let body = c
            .take(len)
            .map_err(|_| InvokeError::Protocol("length exceeds message"))?;
        let padding = c.remaining();
        if !(12..=1024).contains(&padding) {
            return Err(InvokeError::Protocol("bad padding length"));
        }
        Ok((msg_id, body.to_vec()))
    }

    fn ack(&mut self, msg_id: i64) {
        if !self.acks.contains(&msg_id) {
            self.acks.push(msg_id);
        }
    }

    fn resend(&mut self, bad_id: i64, events: &mut Vec<Event>) -> Result<(), InvokeError> {
        let Some(mut pending) = self.pending.remove(&bad_id) else {
            return Ok(());
        };
        if pending.resends >= 3 {
            if let Some(reply) = pending.reply.take() {
                events.push(Event::Reply {
                    reply: Some(reply),
                    result: Err(InvokeError::Protocol("message rejected too many times")),
                });
            }
            return Ok(());
        }
        pending.resends += 1;
        let id = self.next_msg_id();
        let seq = self.next_seq(true);
        let frame = self.seal(id, seq, &pending.body)?;
        self.pending.insert(id, pending);
        events.push(Event::Send(frame));
        Ok(())
    }

    fn complete(
        &mut self,
        req_id: i64,
        result: Result<Vec<u8>, InvokeError>,
        events: &mut Vec<Event>,
    ) {
        if let Some(pending) = self.pending.remove(&req_id) {
            events.push(Event::Reply {
                reply: pending.reply,
                result,
            });
        }
    }

    fn dispatch(
        &mut self,
        body: &[u8],
        envelope_id: i64,
        depth: u32,
        events: &mut Vec<Event>,
    ) -> Result<(), InvokeError> {
        if depth > 8 {
            return Err(InvokeError::Protocol("messages nested too deeply"));
        }
        let mut c = Cursor::new(body);
        let ctor = c.peek_u32()?;
        match ctor {
            CONTAINER_ID => {
                c.read_u32()?;
                let count = c.read_i32()?;
                let count = usize::try_from(count)
                    .ok()
                    .filter(|n| *n <= MAX_CONTAINER_MESSAGES && *n <= c.remaining() / 16)
                    .ok_or(InvokeError::Protocol("container size"))?;
                for _ in 0..count {
                    let inner_id = i64::from_le_bytes(
                        c.take(8)?
                            .try_into()
                            .map_err(|_| InvokeError::Protocol("container"))?,
                    );
                    let _seqno = c.read_i32()?;
                    let len = usize::try_from(c.read_i32()?)
                        .map_err(|_| InvokeError::Protocol("container"))?;
                    let inner = c.take(len)?;
                    self.ack(inner_id);
                    self.dispatch(inner, inner_id, depth + 1, events)?;
                }
                Ok(())
            }
            GZIP_ID => {
                let packed = tl::read_bytes(&mut c)?;
                let inner = gunzip(&packed)?;
                self.dispatch(&inner, envelope_id, depth + 1, events)
            }
            RPC_RESULT_ID => {
                c.read_u32()?;
                let req_id = i64::from_le_bytes(
                    c.take(8)?
                        .try_into()
                        .map_err(|_| InvokeError::Protocol("rpc_result"))?,
                );
                let rest = c.take_rest();
                let result = match Cursor::new(rest).peek_u32() {
                    Ok(GZIP_ID) => {
                        let inner = rest.get(4..).ok_or(InvokeError::Protocol("rpc_result"))?;
                        let packed = tl::read_bytes(&mut Cursor::new(inner))?;
                        Ok(gunzip(&packed)?)
                    }
                    Ok(RPC_ERROR_ID) => Err(rpc_error_of(rest)?),
                    _ => Ok(rest.to_vec()),
                };
                self.complete(req_id, result, events);
                self.ack(envelope_id);
                Ok(())
            }
            <types::MsgsAck as Identifiable>::CONSTRUCTOR_ID => {
                self.ack(envelope_id);
                Ok(())
            }
            <types::NewSessionCreated as Identifiable>::CONSTRUCTOR_ID => {
                let created = <types::NewSessionCreated as Deserializable>::deserialize(&mut c)?;
                self.salt = created.server_salt;
                events.push(Event::Update(UpdateFrame::Gap));
                self.ack(envelope_id);
                Ok(())
            }
            <types::BadServerSalt as Identifiable>::CONSTRUCTOR_ID => {
                let bad = <types::BadServerSalt as Deserializable>::deserialize(&mut c)?;
                self.salt = bad.new_server_salt;
                self.resend(bad.bad_msg_id, events)?;
                self.ack(envelope_id);
                Ok(())
            }
            <types::BadMsgNotification as Identifiable>::CONSTRUCTOR_ID => {
                let bad = <types::BadMsgNotification as Deserializable>::deserialize(&mut c)?;
                match bad.error_code {
                    16 | 17 => {
                        let local = (now_micros() / 1_000_000) as i64;
                        self.time_offset = (envelope_id >> 32) - local;
                        self.resend(bad.bad_msg_id, events)?;
                    }
                    32 => {
                        self.content_count = self.content_count.saturating_add(1);
                        self.resend(bad.bad_msg_id, events)?;
                    }
                    33 => {
                        self.content_count = self.content_count.saturating_sub(1).max(0);
                        self.resend(bad.bad_msg_id, events)?;
                    }
                    _ => self.complete(
                        bad.bad_msg_id,
                        Err(InvokeError::Protocol("server rejected the message")),
                        events,
                    ),
                }
                self.ack(envelope_id);
                Ok(())
            }
            <types::Pong as Identifiable>::CONSTRUCTOR_ID => {
                let pong = <types::Pong as Deserializable>::deserialize(&mut Cursor::new(body))?;
                self.complete(pong.msg_id, Ok(body.to_vec()), events);
                self.ack(envelope_id);
                Ok(())
            }
            FUTURE_SALTS_ID
            | MSG_DETAILED_INFO_ID
            | MSG_NEW_DETAILED_INFO_ID
            | MSGS_STATE_INFO_ID => {
                self.ack(envelope_id);
                Ok(())
            }
            other => {
                self.ack(envelope_id);
                events.push(Event::Update(UpdateFrame::Raw {
                    constructor: other,
                    body: body.to_vec(),
                }));
                Ok(())
            }
        }
    }
}

/// A request to the session task.
enum Command {
    Invoke { body: Vec<u8>, reply: Reply },
}

/// A live encrypted session. Clone-free handle: requests go to the task that owns the socket.
pub struct MtSession {
    commands: mpsc::UnboundedSender<Command>,
}

impl MtSession {
    /// Start the session task on a transport that has completed its handshake. Updates arrive on
    /// the returned receiver.
    pub fn start(
        transport: Transport,
        auth_key: AuthKey,
        salt: i64,
        time_offset: i64,
    ) -> Result<(MtSession, mpsc::UnboundedReceiver<UpdateFrame>), InvokeError> {
        let core = Core::new(&auth_key, salt, time_offset)?;
        drop(auth_key);
        let (reader, writer) = transport.split();
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (update_tx, update_rx) = mpsc::unbounded_channel();
        tokio::spawn(run(reader, writer, core, command_rx, update_tx));
        Ok((
            MtSession {
                commands: command_tx,
            },
            update_rx,
        ))
    }

    /// Send a request and wait for its answer.
    pub async fn invoke<R: RemoteCall>(&self, request: &R) -> Result<R::Return, InvokeError> {
        let mut body = Vec::new();
        request.serialize(&mut body);
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(Command::Invoke { body, reply })
            .map_err(|_| InvokeError::Closed)?;
        let bytes = match tokio::time::timeout(INVOKE_TIMEOUT, answer).await {
            Ok(Ok(result)) => result?,
            Ok(Err(_)) => return Err(InvokeError::Closed),
            Err(_) => return Err(InvokeError::Timeout),
        };
        let mut cursor = Cursor::new(&bytes);
        Ok(<R::Return as Deserializable>::deserialize(&mut cursor)?)
    }
}

async fn send_frame(writer: &mut TransportWriter, frame: &[u8]) -> Result<(), InvokeError> {
    writer.send(frame).await.map_err(InvokeError::Transport)
}

async fn apply(
    writer: &mut TransportWriter,
    updates: &mpsc::UnboundedSender<UpdateFrame>,
    events: Vec<Event>,
) -> Result<(), InvokeError> {
    for event in events {
        match event {
            Event::Send(frame) => send_frame(writer, &frame).await?,
            Event::Reply { reply, result } => {
                if let Some(reply) = reply {
                    let _ = reply.send(result);
                }
            }
            Event::Update(update) => {
                let _ = updates.send(update);
            }
        }
    }
    Ok(())
}

/// The session task: owns the writer and the protocol state; a reader task owns the read half.
async fn run(
    mut reader: TransportReader,
    mut writer: TransportWriter,
    mut core: Core,
    mut commands: mpsc::UnboundedReceiver<Command>,
    updates: mpsc::UnboundedSender<UpdateFrame>,
) {
    let (frames_tx, mut frames_rx) = mpsc::unbounded_channel::<Result<Vec<u8>, TransportError>>();
    let reader_task = tokio::spawn(async move {
        loop {
            let frame = reader.recv().await;
            let failed = frame.is_err();
            if frames_tx.send(frame).is_err() || failed {
                break;
            }
        }
    });
    let mut keepalive =
        tokio::time::interval_at(tokio::time::Instant::now() + KEEPALIVE, KEEPALIVE);
    loop {
        let step: Result<(), InvokeError> = tokio::select! {
            command = commands.recv() => match command {
                None => break,
                Some(Command::Invoke { body, reply }) => match core.request(body, Some(reply)) {
                    Ok(frames) => {
                        let mut result = Ok(());
                        for frame in frames {
                            if let Err(e) = send_frame(&mut writer, &frame).await {
                                result = Err(e);
                                break;
                            }
                        }
                        result
                    }
                    Err(e) => Err(e),
                },
            },
            frame = frames_rx.recv() => match frame {
                None => break,
                Some(Err(e)) => Err(InvokeError::Transport(e)),
                Some(Ok(frame)) => match core.handle(&frame) {
                    Ok(events) => apply(&mut writer, &updates, events).await,
                    Err(e) => Err(e),
                },
            },
            _ = keepalive.tick() => match core.keepalive() {
                Ok(frame) => send_frame(&mut writer, &frame).await,
                Err(e) => Err(e),
            },
        };
        let step = match step {
            Ok(()) => match core.flush_acks() {
                Ok(Some(frame)) => send_frame(&mut writer, &frame).await,
                Ok(None) => Ok(()),
                Err(e) => Err(e),
            },
            Err(e) => Err(e),
        };
        if step.is_err() {
            break;
        }
    }
    reader_task.abort();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> AuthKey {
        let mut bytes = [0u8; 256];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = (i * 7 + 3) as u8;
        }
        AuthKey::from_bytes(bytes)
    }

    /// A frame as the server would send it, for a given session.
    fn server_frame(core: &Core, msg_id: i64, body: &[u8]) -> Vec<u8> {
        let mut plain = Vec::new();
        plain.extend_from_slice(&core.salt.to_le_bytes());
        plain.extend_from_slice(&core.session_id.to_le_bytes());
        plain.extend_from_slice(&msg_id.to_le_bytes());
        plain.extend_from_slice(&1i32.to_le_bytes());
        plain.extend_from_slice(&(body.len() as i32).to_le_bytes());
        plain.extend_from_slice(body);
        let pad = crypto::padding_len(plain.len(), 5);
        plain.extend(vec![0xabu8; pad]);
        let (msg_key, cipher) = crypto::encrypt_message(&core.key, SERVER_X, &plain).unwrap();
        let mut frame = core.key_id.to_le_bytes().to_vec();
        frame.extend_from_slice(&msg_key);
        frame.extend_from_slice(&cipher);
        frame
    }

    /// Open a frame the client sent, as the server would.
    fn open_client(core: &Core, frame: &[u8]) -> (i64, i32, Vec<u8>) {
        let msg_key: [u8; 16] = frame[8..24].try_into().unwrap();
        let plain = crypto::decrypt_message(&core.key, CLIENT_X, &msg_key, &frame[24..]).unwrap();
        let mut c = Cursor::new(&plain);
        c.take(16).unwrap();
        let msg_id = i64::from_le_bytes(c.take(8).unwrap().try_into().unwrap());
        let seq = c.read_i32().unwrap();
        let len = c.read_i32().unwrap() as usize;
        (msg_id, seq, c.take(len).unwrap().to_vec())
    }

    fn server_now_id(offset: i64) -> i64 {
        msg_id_now(offset) | 1 // server ids are odd
    }

    #[test]
    fn msg_ids_are_divisible_by_four_with_a_nonzero_low_word() {
        for micros in [1u128, 999_999, 1_700_000_000_123_456, 1_700_000_000_000_000] {
            let id = msg_id_at(micros);
            assert_eq!(id & 3, 0);
            assert_ne!(id & 0xffff_ffff, 0);
            assert_eq!(id >> 32, (micros / 1_000_000) as i64);
        }
    }

    #[test]
    fn requests_are_sealed_with_increasing_ids_and_odd_content_seqnos() {
        let mut core = Core::new(&key(), 77, 0).unwrap();
        let first = core.request(vec![1, 0, 0, 0], None).unwrap();
        let second = core.request(vec![2, 0, 0, 0], None).unwrap();
        let (id1, seq1, body1) = open_client(&core, &first[0]);
        let (id2, seq2, _) = open_client(&core, &second[0]);
        assert!(id2 > id1);
        assert_eq!(id1 & 3, 0);
        assert_eq!(seq1 & 1, 1);
        assert_eq!(seq2 & 1, 1);
        assert_eq!(seq2, seq1 + 2);
        assert_eq!(body1, vec![1, 0, 0, 0]);
    }

    #[test]
    fn rpc_result_completes_the_matching_request() {
        let mut core = Core::new(&key(), 77, 0).unwrap();
        let (tx, mut rx) = oneshot::channel();
        let frames = core.request(vec![9, 9, 9, 9], Some(tx)).unwrap();
        let (sent_id, _, _) = open_client(&core, &frames[0]);
        let mut result = RPC_RESULT_ID.to_le_bytes().to_vec();
        result.extend_from_slice(&sent_id.to_le_bytes());
        result.extend_from_slice(&[5, 6, 7, 8]);
        let frame = server_frame(&core, server_now_id(0), &result);
        let events = core.handle(&frame).unwrap();
        for event in events {
            if let Event::Reply { reply, result } = event {
                let _ = reply.unwrap().send(result);
            }
        }
        assert_eq!(rx.try_recv().unwrap().unwrap(), vec![5, 6, 7, 8]);
    }

    #[test]
    fn gzipped_rpc_result_is_inflated_within_the_cap() {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;
        let mut core = Core::new(&key(), 77, 0).unwrap();
        let (tx, mut rx) = oneshot::channel();
        let frames = core.request(vec![1, 2, 3, 4], Some(tx)).unwrap();
        let (sent_id, _, _) = open_client(&core, &frames[0]);
        let payload = vec![0x42u8; 5000];
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&payload).unwrap();
        let packed = enc.finish().unwrap();
        let mut gz = GZIP_ID.to_le_bytes().to_vec();
        tl::write_bytes(&mut gz, &packed);
        let mut result = RPC_RESULT_ID.to_le_bytes().to_vec();
        result.extend_from_slice(&sent_id.to_le_bytes());
        result.extend_from_slice(&gz);
        let frame = server_frame(&core, server_now_id(0), &result);
        for event in core.handle(&frame).unwrap() {
            if let Event::Reply { reply, result } = event {
                let _ = reply.unwrap().send(result);
            }
        }
        assert_eq!(rx.try_recv().unwrap().unwrap(), payload);
    }

    #[test]
    fn flood_wait_and_migrate_are_typed() {
        let mut core = Core::new(&key(), 77, 0).unwrap();
        let (tx, mut rx) = oneshot::channel();
        let frames = core.request(vec![0, 0, 0, 0], Some(tx)).unwrap();
        let (sent_id, _, _) = open_client(&core, &frames[0]);
        let mut error = RPC_ERROR_ID.to_le_bytes().to_vec();
        error.extend_from_slice(&420i32.to_le_bytes());
        tl::write_bytes(&mut error, b"FLOOD_WAIT_30");
        let mut result = RPC_RESULT_ID.to_le_bytes().to_vec();
        result.extend_from_slice(&sent_id.to_le_bytes());
        result.extend_from_slice(&error);
        for event in core
            .handle(&server_frame(&core, server_now_id(0), &result))
            .unwrap()
        {
            if let Event::Reply { reply, result } = event {
                let _ = reply.unwrap().send(result);
            }
        }
        assert!(matches!(
            rx.try_recv().unwrap(),
            Err(InvokeError::RateLimited { seconds: 30 })
        ));
    }

    #[test]
    fn bad_server_salt_adopts_the_salt_and_resends_with_a_new_id() {
        let mut core = Core::new(&key(), 77, 0).unwrap();
        let (tx, _rx) = oneshot::channel();
        let frames = core.request(vec![3, 3, 3, 3], Some(tx)).unwrap();
        let (sent_id, _, _) = open_client(&core, &frames[0]);
        let mut bad = BAD_SERVER_SALT_ID.to_le_bytes().to_vec();
        bad.extend_from_slice(&sent_id.to_le_bytes());
        bad.extend_from_slice(&1i32.to_le_bytes());
        bad.extend_from_slice(&48i32.to_le_bytes());
        bad.extend_from_slice(&999i64.to_le_bytes());
        let events = core
            .handle(&server_frame(&core, server_now_id(0), &bad))
            .unwrap();
        let resent = events
            .iter()
            .find_map(|e| match e {
                Event::Send(frame) => Some(frame.clone()),
                _ => None,
            })
            .expect("a resend");
        assert_eq!(core.salt, 999);
        let (new_id, _, body) = open_client(&core, &resent);
        assert_ne!(new_id, sent_id);
        assert_eq!(body, vec![3, 3, 3, 3]);
    }

    #[test]
    fn bad_msg_16_moves_the_clock_to_the_server_time() {
        let mut core = Core::new(&key(), 77, 0).unwrap();
        let (tx, _rx) = oneshot::channel();
        let frames = core.request(vec![4, 4, 4, 4], Some(tx)).unwrap();
        let (sent_id, _, _) = open_client(&core, &frames[0]);
        let mut bad = BAD_MSG_ID.to_le_bytes().to_vec();
        bad.extend_from_slice(&sent_id.to_le_bytes());
        bad.extend_from_slice(&1i32.to_le_bytes());
        bad.extend_from_slice(&16i32.to_le_bytes());
        // The envelope claims the server clock is 3600 s ahead.
        let server_secs = (now_micros() / 1_000_000) as i64 + 3600;
        let envelope = (server_secs << 32) | 1;
        let events = core.handle(&server_frame(&core, envelope, &bad)).unwrap();
        assert!(events.iter().any(|e| matches!(e, Event::Send(_))));
        assert!(
            (core.time_offset - 3600).abs() <= 2,
            "offset {}",
            core.time_offset
        );
    }

    #[test]
    fn container_with_several_answers_is_fully_handled() {
        let mut core = Core::new(&key(), 77, 0).unwrap();
        let (tx1, mut rx1) = oneshot::channel();
        let (tx2, mut rx2) = oneshot::channel();
        let f1 = core.request(vec![1, 1, 1, 1], Some(tx1)).unwrap();
        let f2 = core.request(vec![2, 2, 2, 2], Some(tx2)).unwrap();
        let (id1, _, _) = open_client(&core, &f1[0]);
        let (id2, _, _) = open_client(&core, &f2[0]);
        let answer = |req: i64, payload: [u8; 4]| {
            let mut m = RPC_RESULT_ID.to_le_bytes().to_vec();
            m.extend_from_slice(&req.to_le_bytes());
            m.extend_from_slice(&payload);
            m
        };
        let mut container = CONTAINER_ID.to_le_bytes().to_vec();
        container.extend_from_slice(&2i32.to_le_bytes());
        for (req, payload) in [(id1, [10u8; 4]), (id2, [20u8; 4])] {
            let inner = answer(req, payload);
            container.extend_from_slice(&(server_now_id(0) + 2).to_le_bytes());
            container.extend_from_slice(&1i32.to_le_bytes());
            container.extend_from_slice(&(inner.len() as i32).to_le_bytes());
            container.extend_from_slice(&inner);
        }
        for event in core
            .handle(&server_frame(&core, server_now_id(0) + 4, &container))
            .unwrap()
        {
            if let Event::Reply { reply, result } = event {
                let _ = reply.unwrap().send(result);
            }
        }
        assert_eq!(rx1.try_recv().unwrap().unwrap(), vec![10, 10, 10, 10]);
        assert_eq!(rx2.try_recv().unwrap().unwrap(), vec![20, 20, 20, 20]);
    }

    #[test]
    fn tampered_or_wrong_session_frames_are_refused() {
        let mut core = Core::new(&key(), 77, 0).unwrap();
        let mut frame = server_frame(&core, server_now_id(0), &[1, 2, 3, 4]);
        let last = frame.len() - 1;
        frame[last] ^= 1;
        assert!(core.handle(&frame).is_err());
        let other_session = {
            let mut c = Core::new(&key(), 77, 0).unwrap();
            c.session_id = core.session_id.wrapping_add(1);
            server_frame(&c, server_now_id(0), &[1, 2, 3, 4])
        };
        assert!(matches!(
            core.handle(&other_session),
            Err(InvokeError::Protocol(_))
        ));
    }

    #[test]
    fn even_server_message_ids_are_refused() {
        let core = Core::new(&key(), 77, 0).unwrap();
        let frame = server_frame(&core, 4 << 32, &[1, 2, 3, 4]);
        let mut core = core;
        assert!(core.handle(&frame).is_err());
    }

    #[test]
    fn unknown_objects_become_raw_updates() {
        let mut core = Core::new(&key(), 77, 0).unwrap();
        let body = [0x78, 0x56, 0x34, 0x12, 9, 9, 9, 9];
        let events = core
            .handle(&server_frame(&core, server_now_id(0), &body))
            .unwrap();
        let raw = events.iter().find_map(|e| match e {
            Event::Update(UpdateFrame::Raw { constructor, body }) => {
                Some((*constructor, body.clone()))
            }
            _ => None,
        });
        assert_eq!(raw, Some((0x1234_5678, body.to_vec())));
    }

    const BAD_SERVER_SALT_ID: u32 = 0xedab_447b;
    const BAD_MSG_ID: u32 = 0xa7ef_f811;
}
