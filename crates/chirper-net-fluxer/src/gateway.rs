//! The Fluxer Gateway: one WebSocket connection that identifies (or resumes), heartbeats and turns
//! dispatch events into bridge events.
//!
//! Port of `fluxerapi/gateway.go` and `connector/events.go`. Two deliberate differences from the
//! Go code:
//!
//! - Reconnection is the runtime's job. One [`run`] call is one connection; when it ends, the
//!   event stream ends with `TransientDisconnect` and the runtime calls `connect` again. The resume
//!   state lives in the login store, so the next connection can still resume.
//! - Frames are read and heartbeats are sent by one loop that never waits on event handling. The
//!   handling (which may download attachments) runs in a processor task, in order. The sequence
//!   number advances only after an event has been handled, so a resume never skips one.
//!
//! Heartbeats track their acknowledgement: a beat that is still unanswered at the next tick means
//! the connection is a zombie, and it is ended.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use log::{debug, warn};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};

use chirper_bridge::model::{Event, Member, PortalRef, RemoteEventRef, RemoteUser, SessionState};
use chirper_bridge::{Error, Result};

use crate::api::Client;
use crate::map::{
    self, Channel, Guild, GuildRef, GuildState, MemberEvent, Message as ApiMessage, MessageAck,
    MessageDelete, MessageDeleteBulk, ReactionEvent, Ready, RecipientEvent, TypingEvent, User,
};

const OP_DISPATCH: i64 = 0;
const OP_HEARTBEAT: i64 = 1;
const OP_IDENTIFY: i64 = 2;
const OP_RESUME: i64 = 6;
const OP_RECONNECT: i64 = 7;
const OP_INVALID_SESSION: i64 = 9;
const OP_HELLO: i64 = 10;
const OP_HEARTBEAT_ACK: i64 = 11;
const OP_LAZY_REQUEST: i64 = 14;

const GATEWAY_VERSION: &str = "1";
/// A connection that dropped less than this long ago may be resumed rather than re-identified.
const RESUME_WINDOW: Duration = Duration::from_secs(60);
/// Bounds the dial, the upgrade and the Hello.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_MESSAGE_BYTES: usize = 64 << 20;
const MAX_FRAME_BYTES: usize = 16 << 20;
/// The longest heartbeat interval a Hello may ask for.
const MAX_HEARTBEAT: Duration = Duration::from_secs(600);

const CLOSE_INVALID_TOKEN: u16 = 4004;
const CLOSE_INVALID_SHARD: u16 = 4010;
const CLOSE_SHARDING_REQUIRED: u16 = 4011;
const CLOSE_INVALID_API_VERSION: u16 = 4012;

/// What the gateway remembers about one login between connections. Shared by every connection of
/// the login, so a resume and the guild and channel directory survive a reconnect.
#[derive(Clone, Debug, Default)]
pub struct LoginState {
    pub session_id: Option<String>,
    pub seq: i64,
    pub dropped_at: Option<Instant>,
    /// Channel id to its guild id; an empty guild id marks a direct chat.
    pub channels: HashMap<String, String>,
    pub guilds: HashMap<String, GuildSeen>,
}

/// A guild's name and icon hash, kept so its channels can show its name and icon.
#[derive(Clone, Debug, Default)]
pub struct GuildSeen {
    pub name: String,
    pub icon: Option<String>,
}

/// Login states by login id.
pub type LoginStore = Arc<Mutex<HashMap<String, LoginState>>>;

/// Everything one connection needs.
pub struct GatewayCtx {
    pub url: String,
    pub client: Client,
    pub login_id: String,
    pub me: User,
    pub tx: UnboundedSender<Event>,
    pub store: LoginStore,
}

/// Why a connection ended.
#[derive(Debug, PartialEq, Eq)]
enum End {
    /// The gateway rejected the token; retrying cannot help.
    Auth,
    /// The gateway refused the connection for a reason retrying cannot fix.
    Fatal(String),
    /// A dropped or invalidated connection; the runtime reconnects.
    Transient(String),
}

/// The heartbeat state: one beat may be outstanding at a time.
#[derive(Debug, Default)]
pub struct Heartbeat {
    awaiting_ack: bool,
}

impl Heartbeat {
    /// Called at each tick. `true` means send a beat; `false` means the previous beat was never
    /// acknowledged and the connection is dead.
    pub fn tick(&mut self) -> bool {
        if self.awaiting_ack {
            return false;
        }
        self.awaiting_ack = true;
        true
    }

    pub fn acknowledged(&mut self) {
        self.awaiting_ack = false;
    }
}

/// The sequence number and session id, shared between the reader and the processor.
#[derive(Clone, Default)]
struct Progress {
    seq: Arc<AtomicI64>,
    session: Arc<Mutex<Option<String>>>,
}

impl Progress {
    fn seq(&self) -> i64 {
        self.seq.load(Ordering::Relaxed)
    }

    fn set_seq(&self, seq: i64) {
        self.seq.store(seq, Ordering::Relaxed);
    }

    fn session(&self) -> Option<String> {
        self.session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_session(&self, session: Option<String>) {
        *self.session.lock().unwrap_or_else(PoisonError::into_inner) = session;
    }
}

/// One dispatch, waiting to be handled in order.
struct Dispatch {
    kind: String,
    seq: i64,
    data: Value,
}

/// The shape of every frame the gateway sends or receives.
#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct Payload {
    op: i64,
    d: Value,
    s: Option<i64>,
    t: Option<String>,
}

/// Run one connection until it ends, then report how it ended.
pub async fn run(ctx: GatewayCtx) {
    let progress = Progress::default();
    let end = connection(&ctx, &progress).await;
    let (state, error) = match &end {
        End::Auth => (
            SessionState::BadCredentials,
            "Fluxer rejected the token".to_owned(),
        ),
        End::Fatal(reason) => (SessionState::UnknownError, reason.clone()),
        End::Transient(reason) => (SessionState::TransientDisconnect, reason.clone()),
    };
    remember_end(&ctx, &progress, &end);
    debug!("fluxer: gateway connection ended: {state:?}");
    let _ = ctx.tx.send(Event::State {
        state,
        error,
        message: String::new(),
        profile: None,
    });
}

fn remember_end(ctx: &GatewayCtx, progress: &Progress, end: &End) {
    with_login(&ctx.store, &ctx.login_id, |state| {
        if matches!(end, End::Auth | End::Fatal(_)) {
            state.session_id = None;
            state.seq = 0;
            state.dropped_at = None;
        } else {
            state.session_id = progress.session();
            state.seq = progress.seq();
            state.dropped_at = Some(Instant::now());
        }
    });
}

fn with_login<R>(store: &LoginStore, login: &str, f: impl FnOnce(&mut LoginState) -> R) -> R {
    let mut map = store.lock().unwrap_or_else(PoisonError::into_inner);
    f(map.entry(login.to_owned()).or_default())
}

/// Dial, read the Hello, authenticate, then pump frames until the socket ends.
async fn connection(ctx: &GatewayCtx, progress: &Progress) -> End {
    let url = match gateway_url_with_params(&ctx.url) {
        Ok(url) => url,
        Err(err) => return End::Fatal(err.to_string()),
    };
    let request = match url.into_client_request() {
        Ok(request) => request,
        Err(_) => return End::Fatal("the gateway URL is not valid".to_owned()),
    };
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_FRAME_BYTES));
    let dial = tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        tokio_tungstenite::connect_async_with_config(request, Some(config), false),
    )
    .await;
    let mut ws = match dial {
        Err(_) => return End::Transient("the gateway handshake timed out".to_owned()),
        Ok(Err(err)) => return End::Transient(format!("gateway connect: {err}")),
        Ok(Ok((ws, _))) => ws,
    };

    let interval = match tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
        loop {
            match ws.next().await {
                None => return Err(End::Transient("the gateway closed before Hello".to_owned())),
                Some(Err(err)) => return Err(End::Transient(format!("gateway read: {err}"))),
                Some(Ok(Message::Close(frame))) => return Err(close_end(frame)),
                Some(Ok(Message::Text(text))) => return hello_interval(text.as_bytes()),
                Some(Ok(Message::Binary(bytes))) => return hello_interval(&bytes),
                Some(Ok(_)) => continue,
            }
        }
    })
    .await
    {
        Err(_) => return End::Transient("the gateway Hello timed out".to_owned()),
        Ok(Err(end)) => return end,
        Ok(Ok(interval)) => interval,
    };

    let first = match take_resume(ctx) {
        Some((session, seq)) => {
            progress.set_session(Some(session.clone()));
            progress.set_seq(seq);
            frame(
                OP_RESUME,
                json!({ "token": ctx.client.credential().token(), "session_id": session, "seq": seq }),
            )
        }
        None => {
            progress.set_session(None);
            progress.set_seq(0);
            frame(OP_IDENTIFY, identify(ctx))
        }
    };
    if let Err(err) = ws.send(Message::Text(first.into())).await {
        return End::Transient(format!("gateway send: {err}"));
    }

    let (work_tx, work_rx) = unbounded_channel::<Dispatch>();
    let (out_tx, mut out_rx) = unbounded_channel::<String>();
    let processor = Processor {
        client: ctx.client.clone(),
        login_id: ctx.login_id.clone(),
        me: ctx.me.clone(),
        media: ctx.client.media_base().to_owned(),
        tx: ctx.tx.clone(),
        out: out_tx.clone(),
        store: ctx.store.clone(),
        progress: progress.clone(),
        bot: ctx.client.credential().is_bot(),
    };
    let handle = tokio::spawn(process(processor, work_rx));

    let end = pump(&mut ws, interval, progress, &work_tx, &out_tx, &mut out_rx).await;
    handle.abort();
    let _ = tokio::time::timeout(Duration::from_secs(2), ws.close(None)).await;
    end
}

/// Read frames and send heartbeats until the connection ends.
async fn pump<S>(
    ws: &mut S,
    interval: Duration,
    progress: &Progress,
    work: &UnboundedSender<Dispatch>,
    out: &UnboundedSender<String>,
    outgoing: &mut UnboundedReceiver<String>,
) -> End
where
    S: SinkExt<Message>
        + StreamExt<Item = std::result::Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await;
    let mut heartbeat = Heartbeat::default();
    loop {
        tokio::select! {
            incoming = ws.next() => {
                let message = match incoming {
                    None => return End::Transient("the gateway closed".to_owned()),
                    Some(Err(err)) => return End::Transient(format!("gateway read: {err}")),
                    Some(Ok(message)) => message,
                };
                match message {
                    Message::Text(text) => {
                        if let Some(end) = handle_frame(text.as_bytes(), progress, work, out, &mut heartbeat) {
                            return end;
                        }
                    }
                    Message::Binary(bytes) => {
                        if let Some(end) = handle_frame(&bytes, progress, work, out, &mut heartbeat) {
                            return end;
                        }
                    }
                    Message::Ping(payload) => {
                        if ws.send(Message::Pong(payload)).await.is_err() {
                            return End::Transient("gateway send failed".to_owned());
                        }
                    }
                    Message::Close(frame) => return close_end(frame),
                    Message::Pong(_) | Message::Frame(_) => {}
                }
            }
            Some(text) = outgoing.recv() => {
                if ws.send(Message::Text(text.into())).await.is_err() {
                    return End::Transient("gateway send failed".to_owned());
                }
            }
            _ = ticker.tick() => {
                if !heartbeat.tick() {
                    return End::Transient("the gateway did not acknowledge a heartbeat".to_owned());
                }
                if ws.send(Message::Text(heartbeat_frame(progress.seq()).into())).await.is_err() {
                    return End::Transient("gateway send failed".to_owned());
                }
            }
        }
    }
}

/// Handle one frame that is not a close. Returns `Some` when the connection must end.
fn handle_frame(
    bytes: &[u8],
    progress: &Progress,
    work: &UnboundedSender<Dispatch>,
    out: &UnboundedSender<String>,
    heartbeat: &mut Heartbeat,
) -> Option<End> {
    let payload: Payload = match serde_json::from_slice(bytes) {
        Ok(payload) => payload,
        Err(_) => {
            warn!("fluxer: ignoring a malformed gateway frame");
            return None;
        }
    };
    match payload.op {
        OP_DISPATCH => {
            if let (Some(seq), Some(kind)) = (payload.s, payload.t) {
                let sent = work.send(Dispatch {
                    kind,
                    seq,
                    data: payload.d,
                });
                if sent.is_err() {
                    return Some(End::Transient("the event processor stopped".to_owned()));
                }
            }
            None
        }
        OP_HEARTBEAT => {
            let _ = out.send(heartbeat_frame(progress.seq()));
            None
        }
        OP_HEARTBEAT_ACK => {
            heartbeat.acknowledged();
            None
        }
        OP_RECONNECT => Some(End::Transient("the gateway asked to reconnect".to_owned())),
        OP_INVALID_SESSION => {
            progress.set_session(None);
            progress.set_seq(0);
            Some(End::Transient(
                "the gateway invalidated the session".to_owned(),
            ))
        }
        OP_HELLO => None,
        _ => None,
    }
}

/// Map a close frame to how the connection ended.
fn close_end(frame: Option<CloseFrame>) -> End {
    let code = frame.map(|f| u16::from(f.code));
    match code {
        Some(CLOSE_INVALID_TOKEN) => End::Auth,
        Some(CLOSE_INVALID_SHARD | CLOSE_SHARDING_REQUIRED | CLOSE_INVALID_API_VERSION) => {
            End::Fatal(format!(
                "the gateway refused the connection (close {})",
                code.unwrap_or(0)
            ))
        }
        Some(code) => End::Transient(format!("the gateway closed the connection (close {code})")),
        None => End::Transient("the gateway closed the connection".to_owned()),
    }
}

/// The heartbeat interval a Hello asks for.
fn hello_interval(bytes: &[u8]) -> std::result::Result<Duration, End> {
    let payload: Payload = serde_json::from_slice(bytes)
        .map_err(|_| End::Transient("malformed gateway Hello".to_owned()))?;
    if payload.op != OP_HELLO {
        return Err(End::Transient(
            "the gateway did not start with Hello".to_owned(),
        ));
    }
    let millis = payload
        .d
        .get("heartbeat_interval")
        .and_then(Value::as_i64)
        .filter(|ms| *ms > 0)
        .ok_or_else(|| End::Transient("malformed gateway Hello".to_owned()))?;
    let interval = Duration::from_millis(u64::try_from(millis).unwrap_or(0));
    Ok(interval.min(MAX_HEARTBEAT))
}

/// The resume state, when the last connection dropped recently enough. Clears it otherwise.
fn take_resume(ctx: &GatewayCtx) -> Option<(String, i64)> {
    with_login(&ctx.store, &ctx.login_id, |state| {
        match (&state.session_id, state.dropped_at) {
            (Some(session), Some(at)) if at.elapsed() < RESUME_WINDOW => {
                Some((session.clone(), state.seq))
            }
            _ => {
                state.session_id = None;
                state.seq = 0;
                None
            }
        }
    })
}

fn identify(ctx: &GatewayCtx) -> Value {
    json!({
        "token": ctx.client.credential().token(),
        "properties": {
            "os": "Android",
            "browser": "Chirper",
            "device": "Chirper",
            "mobile": true,
        }
    })
}

fn heartbeat_frame(seq: i64) -> String {
    let d = if seq == 0 { Value::Null } else { json!(seq) };
    json!({ "op": OP_HEARTBEAT, "d": d }).to_string()
}

fn frame(op: i64, d: Value) -> String {
    json!({ "op": op, "d": d }).to_string()
}

/// The gateway URL with the connection parameters added. The URL's own query is kept.
pub fn gateway_url_with_params(raw: &str) -> Result<String> {
    let mut url = url::Url::parse(raw)
        .map_err(|_| Error::Protocol("the gateway URL is not valid".to_owned()))?;
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| key != "v" && key != "encoding")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    {
        let mut query = url.query_pairs_mut();
        query.clear();
        for (key, value) in &kept {
            query.append_pair(key, value);
        }
        query.append_pair("v", GATEWAY_VERSION);
        query.append_pair("encoding", "json");
    }
    Ok(url.to_string())
}

/// Everything the processor needs to turn dispatches into bridge events.
struct Processor {
    client: Client,
    login_id: String,
    me: User,
    media: String,
    tx: UnboundedSender<Event>,
    out: UnboundedSender<String>,
    store: LoginStore,
    progress: Progress,
    bot: bool,
}

/// Handle dispatches in order. The sequence number advances after each one.
async fn process(p: Processor, mut work: UnboundedReceiver<Dispatch>) {
    while let Some(dispatch) = work.recv().await {
        handle(&p, &dispatch).await;
        p.progress.set_seq(dispatch.seq);
    }
}

impl Processor {
    fn emit(&self, event: Event) {
        let _ = self.tx.send(event);
    }

    fn me_id(&self) -> &str {
        &self.me.id
    }

    /// The guild a channel belongs to: the payload's guild, else the one the directory remembers.
    fn guild_of(&self, channel: &str, guild: &str) -> Option<String> {
        if !guild.is_empty() {
            return Some(guild.to_owned());
        }
        with_login(&self.store, &self.login_id, |state| {
            state
                .channels
                .get(channel)
                .filter(|g| !g.is_empty())
                .cloned()
        })
    }

    fn portal(&self, channel: &str, guild: &str) -> PortalRef {
        let guild = self.guild_of(channel, guild);
        map::channel_portal(channel, guild.as_deref(), &self.login_id)
    }

    fn remember_channel(&self, channel: &str, guild: &str) {
        with_login(&self.store, &self.login_id, |state| {
            state.channels.insert(channel.to_owned(), guild.to_owned());
        });
    }

    fn remember_guild(&self, id: &str, name: &str, icon: Option<&str>) {
        with_login(&self.store, &self.login_id, |state| {
            state.guilds.insert(
                id.to_owned(),
                GuildSeen {
                    name: name.to_owned(),
                    icon: icon.map(str::to_owned),
                },
            );
        });
    }

    fn guild_seen(&self, id: &str) -> GuildSeen {
        with_login(&self.store, &self.login_id, |state| {
            state.guilds.get(id).cloned().unwrap_or_default()
        })
    }

    fn chat(&self, channel: &Channel, guild: &str) {
        let seen = if guild.is_empty() {
            GuildSeen::default()
        } else {
            self.guild_seen(guild)
        };
        let parent = (!guild.is_empty()).then(|| map::Parent {
            id: guild,
            name: &seen.name,
            icon: seen.icon.as_deref(),
        });
        if let Some(meta) = map::channel_meta(channel, parent, &self.login_id, &self.media) {
            self.emit(Event::Chat(meta));
        }
    }

    fn space(&self, guild: &Guild) {
        self.remember_guild(&guild.id, &guild.name, guild.icon.as_deref());
        self.emit(Event::Chat(map::guild_meta(guild, &self.media)));
    }

    /// A guild as a space, with every channel of it that is a chat.
    fn guild_state(&self, guild: &GuildState) {
        let properties = Guild {
            id: guild.id.clone(),
            ..guild.properties.clone()
        };
        self.space(&properties);
        for channel in &guild.channels {
            self.remember_channel(&channel.id, &guild.id);
            self.chat(channel, &guild.id);
        }
    }

    fn subscribe(&self, guilds: &[String]) {
        if self.bot || guilds.is_empty() {
            return;
        }
        let subscriptions: serde_json::Map<String, Value> = guilds
            .iter()
            .map(|id| (id.clone(), json!({ "active": true, "typing": true })))
            .collect();
        let _ = self.out.send(frame(
            OP_LAZY_REQUEST,
            json!({ "subscriptions": subscriptions }),
        ));
    }

    fn remote_user(&self, user: &User) -> RemoteUser {
        map::remote_user(user, &self.media)
    }
}

/// Decode a payload, or log that the event was ignored. The payload itself is never logged.
fn decode<T: DeserializeOwned>(kind: &str, data: &Value) -> Option<T> {
    match serde_json::from_value(data.clone()) {
        Ok(value) => Some(value),
        Err(_) => {
            warn!("fluxer: ignoring a malformed {kind} event");
            None
        }
    }
}

async fn handle(p: &Processor, dispatch: &Dispatch) {
    let kind = dispatch.kind.as_str();
    let data = &dispatch.data;
    match kind {
        "READY" => {
            let Some(ready) = decode::<Ready>(kind, data) else {
                return;
            };
            p.progress
                .set_session(Some(ready.session_id.clone()).filter(|s| !s.is_empty()));
            let profile = (!ready.user.id.is_empty()).then(|| p.remote_user(&ready.user));
            p.emit(Event::State {
                state: SessionState::Running,
                error: String::new(),
                message: String::new(),
                profile,
            });
            for channel in &ready.private_channels {
                p.remember_channel(&channel.id, "");
                p.chat(channel, "");
            }
            let mut guilds = Vec::new();
            for guild in &ready.guilds {
                if guild.unavailable {
                    continue;
                }
                guilds.push(guild.id.clone());
                p.guild_state(guild);
            }
            p.subscribe(&guilds);
        }
        "RESUMED" => p.emit(Event::State {
            state: SessionState::Running,
            error: String::new(),
            message: String::new(),
            profile: None,
        }),
        "GUILD_CREATE" => {
            let Some(guild) = decode::<GuildState>(kind, data) else {
                return;
            };
            if guild.unavailable || guild.id.is_empty() {
                return;
            }
            p.guild_state(&guild);
            p.subscribe(std::slice::from_ref(&guild.id));
        }
        "GUILD_UPDATE" => {
            let Some(guild) = decode::<Guild>(kind, data) else {
                return;
            };
            if !guild.id.is_empty() {
                p.space(&guild);
            }
        }
        "GUILD_DELETE" => {
            let Some(guild) = decode::<GuildRef>(kind, data) else {
                return;
            };
            if guild.unavailable || guild.id.is_empty() {
                return;
            }
            let children: Vec<String> = with_login(&p.store, &p.login_id, |state| {
                let children = state
                    .channels
                    .iter()
                    .filter(|(_, g)| **g == guild.id)
                    .map(|(c, _)| c.clone())
                    .collect();
                state.channels.retain(|_, g| *g != guild.id);
                state.guilds.remove(&guild.id);
                children
            });
            for child in children {
                p.emit(Event::ChatRemoved(map::channel_portal(
                    &child,
                    Some(&guild.id),
                    &p.login_id,
                )));
            }
            p.emit(Event::ChatRemoved(map::guild_portal(&guild.id)));
        }
        "CHANNEL_CREATE" | "CHANNEL_UPDATE" => {
            let Some(channel) = decode::<Channel>(kind, data) else {
                return;
            };
            if channel.id.is_empty() {
                return;
            }
            p.remember_channel(&channel.id, &channel.guild_id);
            p.chat(&channel, &channel.guild_id);
        }
        "CHANNEL_DELETE" => {
            let Some(channel) = decode::<Channel>(kind, data) else {
                return;
            };
            if channel.id.is_empty() {
                return;
            }
            let portal = p.portal(&channel.id, &channel.guild_id);
            with_login(&p.store, &p.login_id, |state| {
                state.channels.remove(&channel.id);
            });
            p.emit(Event::ChatRemoved(portal));
        }
        "MESSAGE_CREATE" => {
            let Some(message) = decode::<ApiMessage>(kind, data) else {
                return;
            };
            if message.id.is_empty()
                || message.channel_id.is_empty()
                || message.author.id.is_empty()
            {
                warn!("fluxer: ignoring a MESSAGE_CREATE without its ids");
                return;
            }
            let portal = p.portal(&message.channel_id, &message.guild_id);
            for part in map::messages_from(&p.client, &message, &portal, p.me_id(), true).await {
                p.emit(Event::Message(part));
            }
        }
        "MESSAGE_UPDATE" => {
            let Some(message) = decode::<ApiMessage>(kind, data) else {
                return;
            };
            if message.id.is_empty() || message.channel_id.is_empty() {
                return;
            }
            let portal = p.portal(&message.channel_id, &message.guild_id);
            if let Some(edit) = map::edit_from(&message, &portal, p.me_id(), &p.media) {
                p.emit(Event::Message(edit));
            }
        }
        "MESSAGE_DELETE" => {
            let Some(delete) = decode::<MessageDelete>(kind, data) else {
                return;
            };
            if delete.id.is_empty() || delete.channel_id.is_empty() {
                return;
            }
            p.emit(Event::Redact {
                portal: p.portal(&delete.channel_id, &delete.guild_id),
                event: RemoteEventRef::new(delete.id),
            });
        }
        "MESSAGE_DELETE_BULK" => {
            let Some(bulk) = decode::<MessageDeleteBulk>(kind, data) else {
                return;
            };
            if bulk.channel_id.is_empty() {
                return;
            }
            let portal = p.portal(&bulk.channel_id, &bulk.guild_id);
            for id in bulk.ids.into_iter().filter(|id| !id.is_empty()) {
                p.emit(Event::Redact {
                    portal: portal.clone(),
                    event: RemoteEventRef::new(id),
                });
            }
        }
        "MESSAGE_REACTION_ADD" | "MESSAGE_REACTION_REMOVE" => {
            let Some(reaction) = decode::<ReactionEvent>(kind, data) else {
                return;
            };
            if reaction.channel_id.is_empty() || reaction.message_id.is_empty() {
                return;
            }
            // The event names one reactor, but the bridge wants the whole set for the message, so the
            // message is read back. Events of one connection are handled one at a time, which caps
            // the reads at one in flight.
            match p
                .client
                .message(&reaction.channel_id, &reaction.message_id)
                .await
            {
                Ok(message) => p.emit(Event::Reactions {
                    portal: p.portal(&reaction.channel_id, &reaction.guild_id),
                    target: RemoteEventRef::new(reaction.message_id),
                    reactions: map::reactions(&message),
                }),
                Err(err) => warn!("fluxer: a reaction could not be read back: {err}"),
            }
        }
        "TYPING_START" => {
            let Some(typing) = decode::<TypingEvent>(kind, data) else {
                return;
            };
            if typing.channel_id.is_empty()
                || typing.user_id.is_empty()
                || typing.user_id == p.me_id()
            {
                return;
            }
            p.emit(Event::Typing {
                portal: p.portal(&typing.channel_id, &typing.guild_id),
                senders: vec![RemoteUser {
                    id: typing.user_id,
                    ..Default::default()
                }],
            });
        }
        "MESSAGE_ACK" => {
            let Some(ack) = decode::<MessageAck>(kind, data) else {
                return;
            };
            if ack.channel_id.is_empty() || ack.message_id.is_empty() {
                return;
            }
            p.emit(Event::ReadReceipt {
                portal: p.portal(&ack.channel_id, ""),
                user: p.remote_user(&p.me),
                event: RemoteEventRef::new(ack.message_id),
            });
        }
        "CHANNEL_RECIPIENT_ADD" | "CHANNEL_RECIPIENT_REMOVE" => {
            let Some(change) = decode::<RecipientEvent>(kind, data) else {
                return;
            };
            if change.channel_id.is_empty() || change.user.id.is_empty() {
                return;
            }
            let membership = if kind == "CHANNEL_RECIPIENT_ADD" {
                "join"
            } else {
                "leave"
            };
            p.emit(Event::Member {
                portal: map::channel_portal(&change.channel_id, None, &p.login_id),
                member: Member {
                    user: p.remote_user(&change.user),
                    membership: membership.to_owned(),
                },
            });
        }
        "GUILD_MEMBER_ADD" | "GUILD_MEMBER_REMOVE" => {
            let Some(change) = decode::<MemberEvent>(kind, data) else {
                return;
            };
            if change.guild_id.is_empty() || change.user.id.is_empty() {
                return;
            }
            let membership = if kind == "GUILD_MEMBER_ADD" {
                "join"
            } else {
                "leave"
            };
            p.emit(Event::Member {
                portal: map::guild_portal(&change.guild_id),
                member: Member {
                    user: p.remote_user(&change.user),
                    membership: membership.to_owned(),
                },
            });
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(json: &str) -> Payload {
        serde_json::from_str(json).expect("test frame is valid JSON")
    }

    #[test]
    fn hello_gives_the_heartbeat_interval() {
        let interval =
            hello_interval(br#"{"op":10,"d":{"heartbeat_interval":41250}}"#).expect("valid Hello");
        assert_eq!(interval, Duration::from_millis(41_250));
    }

    #[test]
    fn malformed_hello_is_refused() {
        assert!(hello_interval(br#"{"op":10,"d":{"heartbeat_interval":0}}"#).is_err());
        assert!(hello_interval(br#"{"op":10,"d":{}}"#).is_err());
        assert!(hello_interval(br#"{"op":0,"d":{"heartbeat_interval":5}}"#).is_err());
        assert!(hello_interval(b"not json").is_err());
        let huge =
            hello_interval(br#"{"op":10,"d":{"heartbeat_interval":99999999}}"#).expect("clamped");
        assert_eq!(huge, MAX_HEARTBEAT);
    }

    #[test]
    fn dispatch_frames_carry_type_sequence_and_data() {
        let p = payload(r#"{"op":0,"t":"MESSAGE_CREATE","s":7,"d":{"id":"100"}}"#);
        assert_eq!(p.op, OP_DISPATCH);
        assert_eq!(p.t.as_deref(), Some("MESSAGE_CREATE"));
        assert_eq!(p.s, Some(7));
        assert_eq!(p.d["id"], "100");
    }

    #[test]
    fn heartbeat_is_null_before_the_first_dispatch() {
        assert_eq!(heartbeat_frame(0), r#"{"d":null,"op":1}"#);
        assert_eq!(heartbeat_frame(12), r#"{"d":12,"op":1}"#);
    }

    #[test]
    fn an_unacknowledged_heartbeat_marks_a_zombie() {
        let mut hb = Heartbeat::default();
        assert!(hb.tick(), "the first beat is sent");
        assert!(!hb.tick(), "the next tick finds the beat unanswered");
        let mut hb = Heartbeat::default();
        assert!(hb.tick());
        hb.acknowledged();
        assert!(hb.tick(), "an acknowledged beat lets the next one go out");
    }

    #[test]
    fn close_codes_map_to_outcomes() {
        let close = |code: u16| {
            Some(CloseFrame {
                code: code.into(),
                reason: "".into(),
            })
        };
        assert_eq!(close_end(close(4004)), End::Auth);
        assert!(matches!(close_end(close(4010)), End::Fatal(_)));
        assert!(matches!(close_end(close(4011)), End::Fatal(_)));
        assert!(matches!(close_end(close(4012)), End::Fatal(_)));
        assert!(matches!(close_end(close(4008)), End::Transient(_)));
        assert!(matches!(close_end(close(4000)), End::Transient(_)));
        assert!(matches!(close_end(None), End::Transient(_)));
    }

    #[test]
    fn a_close_reason_is_not_repeated_in_the_outcome() {
        let end = close_end(Some(CloseFrame {
            code: 4004u16.into(),
            reason: "token secret-value".into(),
        }));
        assert_eq!(end, End::Auth);
    }

    #[test]
    fn gateway_url_keeps_its_query_and_adds_the_version() {
        let got = gateway_url_with_params("wss://gw.example.test/?shard=x").expect("valid URL");
        for want in ["v=1", "encoding=json", "shard=x"] {
            assert!(got.contains(want), "{got} is missing {want}");
        }
        let replaced =
            gateway_url_with_params("wss://gw.example.test/?v=9&encoding=etf").expect("valid");
        assert!(replaced.contains("v=1") && replaced.contains("encoding=json"));
        assert!(!replaced.contains("v=9") && !replaced.contains("etf"));
    }

    #[test]
    fn resume_frame_names_the_session_and_sequence() {
        let text = frame(
            OP_RESUME,
            json!({ "token": "t", "session_id": "sess-1", "seq": 2 }),
        );
        let value: Value = serde_json::from_str(&text).expect("JSON");
        assert_eq!(value["op"], 6);
        assert_eq!(value["d"]["session_id"], "sess-1");
        assert_eq!(value["d"]["seq"], 2);
    }

    #[test]
    fn progress_is_shared_between_clones() {
        let progress = Progress::default();
        let other = progress.clone();
        progress.set_seq(5);
        progress.set_session(Some("s".to_owned()));
        assert_eq!(other.seq(), 5);
        assert_eq!(other.session().as_deref(), Some("s"));
    }

    #[tokio::test]
    async fn pump_ends_on_an_unanswered_heartbeat() {
        // A socket that never answers: the first beat stays unacknowledged, so the next tick ends it.
        let progress = Progress::default();
        let (work_tx, _work_rx) = unbounded_channel();
        let (out_tx, mut out_rx) = unbounded_channel();
        let mut ws = SilentSocket;
        let end = pump(
            &mut ws,
            Duration::from_millis(5),
            &progress,
            &work_tx,
            &out_tx,
            &mut out_rx,
        )
        .await;
        assert!(matches!(end, End::Transient(reason) if reason.contains("acknowledge")));
    }

    /// A stream that never yields and accepts every write.
    #[derive(Default)]
    struct SilentSocket;

    impl futures_util::Stream for SilentSocket {
        type Item = std::result::Result<Message, tokio_tungstenite::tungstenite::Error>;

        fn poll_next(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Self::Item>> {
            std::task::Poll::Pending
        }
    }

    impl futures_util::Sink<Message> for SilentSocket {
        type Error = tokio_tungstenite::tungstenite::Error;

        fn poll_ready(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::result::Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn start_send(
            self: std::pin::Pin<&mut Self>,
            _item: Message,
        ) -> std::result::Result<(), Self::Error> {
            Ok(())
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::result::Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn poll_close(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::result::Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }
    }
}
