//! The Discord gateway: one WebSocket connection that turns dispatch events into bridge events.
//!
//! Reconnection is the runtime's job. A connection resumes the previous session when the driver
//! has one stored, keeps the heartbeat, forwards events and ends when the socket closes. The resume
//! state lives in the driver, so it survives the runtime calling `connect` again.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{Sink, SinkExt, Stream, StreamExt};
use log::{debug, warn};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};

use chirper_bridge::model::{Event, Member, PortalRef, Reaction, RemoteEventRef, SessionState};

use crate::api::Api;
use crate::map::{self, Channel, Guild, Message, MessageUpdate, ReactionEvent, User};

/// The user-account intents, bit by bit: GUILDS (1), GUILD_MESSAGES (512), GUILD_MESSAGE_REACTIONS
/// (1024), GUILD_MESSAGE_TYPING (2048), DIRECT_MESSAGES (4096), DIRECT_MESSAGE_REACTIONS (8192),
/// DIRECT_MESSAGE_TYPING (16384) and MESSAGE_CONTENT (32768, privileged: the bot must be allowed
/// it in the developer portal).
pub const USER_INTENTS: u64 = 1 | 512 | 1024 | 2048 | 4096 | 8192 | 16384 | 32768;
/// The bot-account intents of the Go connector: the user set plus GUILD_MEMBERS (2, privileged).
pub const BOT_INTENTS: u64 = USER_INTENTS | 2;
/// Largest gateway message and frame accepted from Discord.
const MAX_MESSAGE_BYTES: usize = 64 << 20;
const MAX_FRAME_BYTES: usize = 16 << 20;
/// Heartbeat interval used when Discord's hello gives none.
const DEFAULT_HEARTBEAT_MS: u64 = 41_250;
/// Reaction summaries kept per connection.
const REACTION_CACHE_LIMIT: usize = 512;

/// What a reconnect needs to resume the session instead of identifying again.
#[derive(Clone, PartialEq, Eq)]
pub struct ResumeState {
    pub session_id: String,
    pub resume_gateway_url: String,
    pub seq: Option<i64>,
}

impl std::fmt::Debug for ResumeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResumeState")
            .field("session_id", &"[redacted]")
            .field("resume_gateway_url", &self.resume_gateway_url)
            .field("seq", &self.seq)
            .finish()
    }
}

/// The resume state of every login of one driver, keyed by login id.
pub type ResumeStore = Arc<Mutex<HashMap<String, ResumeState>>>;

/// Everything one gateway connection needs from its login.
pub struct Link {
    pub api: Api,
    /// The authorization value: the token, with the `Bot ` prefix for a bot account.
    pub token: String,
    /// The intents to identify with: [`USER_INTENTS`] or [`BOT_INTENTS`].
    pub intents: u64,
    /// The local user's id, to tell the user's own reactions apart.
    pub me_id: String,
    /// This login's key in the resume store.
    pub login_id: String,
    pub resume: ResumeStore,
    pub tx: UnboundedSender<Event>,
}

/// Reaction summaries of recent messages, so a reaction event can be applied without a read.
#[derive(Default)]
pub struct ReactionCache {
    entries: HashMap<(String, String), Vec<Reaction>>,
    order: VecDeque<(String, String)>,
}

impl ReactionCache {
    /// Store the summary of a message, evicting the oldest entries when the cache is full.
    pub fn store(&mut self, key: (String, String), reactions: Vec<Reaction>) {
        if self.entries.insert(key.clone(), reactions).is_none() {
            self.order.push_back(key);
        }
        while self.entries.len() > REACTION_CACHE_LIMIT {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }

    /// Change a cached summary in place. Returns the new summary, or `None` when the message is
    /// not cached.
    pub fn change(
        &mut self,
        key: &(String, String),
        change: impl FnOnce(&mut Vec<Reaction>),
    ) -> Option<Vec<Reaction>> {
        let entry = self.entries.get_mut(key)?;
        change(entry);
        Some(entry.clone())
    }
}

/// Apply one reaction event to a summary: the count moves by `delta`, the emoji is dropped at zero,
/// and `from_me` follows the local user's own events.
pub fn apply_delta(reactions: &mut Vec<Reaction>, key: &str, delta: i64, from_me: bool) {
    match reactions.iter().position(|reaction| reaction.key == key) {
        Some(index) => {
            let entry = &mut reactions[index];
            entry.count += delta;
            if from_me {
                entry.from_me = delta > 0;
            }
            if entry.count <= 0 {
                reactions.remove(index);
            }
        }
        None if delta > 0 => reactions.push(Reaction {
            key: key.to_owned(),
            count: 1,
            from_me,
            senders: Vec::new(),
        }),
        None => {}
    }
}

/// How the connection ended, and what the runtime should be told.
#[derive(Debug, PartialEq, Eq)]
enum Ending {
    /// The socket ended for a reason the runtime can recover from by reconnecting.
    Transient,
    /// The login cannot go on as it is.
    Stop { state: SessionState, error: String },
}

struct Heartbeat {
    interval: Duration,
    next: Instant,
    acked: bool,
}

/// One gateway connection and everything its events need.
struct Conn {
    api: Api,
    token: String,
    intents: u64,
    me_id: String,
    login_id: String,
    resume: ResumeStore,
    tx: UnboundedSender<Event>,
    reactions: Arc<Mutex<ReactionCache>>,
    sequence: Option<i64>,
}

/// Run one gateway connection. Returns when the socket ends.
pub async fn run(link: Link, gateway_url: String) {
    let mut conn = Conn::new(link, None);
    let stored = conn.resume_state();
    conn.sequence = stored.as_ref().and_then(|state| state.seq);
    let base = stored
        .as_ref()
        .map_or(gateway_url, |state| state.resume_gateway_url.clone());
    let url = format!("{}/?v=10&encoding=json", base.trim_end_matches('/'));
    let ending = match open(&url).await {
        Ok(socket) => conn.drive(socket, stored).await,
        Err(message) => {
            warn!("discord: {message}");
            Ending::Stop {
                state: SessionState::TransientDisconnect,
                error: message,
            }
        }
    };
    conn.finish(ending);
}

/// Open the socket with the frame limits of this driver.
async fn open(
    url: &str,
) -> Result<impl Stream<Item = Result<WsMessage, WsError>> + Sink<WsMessage> + Unpin, String> {
    let request = url
        .into_client_request()
        .map_err(|err| format!("gateway request: {err}"))?;
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_FRAME_BYTES));
    let (socket, _response) =
        tokio_tungstenite::connect_async_with_config(request, Some(config), false)
            .await
            .map_err(|err| format!("gateway connect: {err}"))?;
    Ok(socket)
}

impl Conn {
    fn new(link: Link, sequence: Option<i64>) -> Self {
        Self {
            api: link.api,
            token: link.token,
            intents: link.intents,
            me_id: link.me_id,
            login_id: link.login_id,
            resume: link.resume,
            tx: link.tx,
            reactions: Arc::default(),
            sequence,
        }
    }

    fn emit(&self, event: Event) {
        let _ = self.tx.send(event);
    }

    fn state(&self, state: SessionState, error: String) {
        self.emit(Event::State {
            state,
            error,
            message: String::new(),
            profile: None,
        });
    }

    fn finish(&self, ending: Ending) {
        match ending {
            Ending::Transient => self.state(SessionState::TransientDisconnect, String::new()),
            Ending::Stop { state, error } => self.state(state, error),
        }
    }

    fn resume_state(&self) -> Option<ResumeState> {
        self.resume.lock().ok()?.get(&self.login_id).cloned()
    }

    fn store_resume(&self, state: ResumeState) {
        if let Ok(mut store) = self.resume.lock() {
            store.insert(self.login_id.clone(), state);
        }
    }

    /// Keep the stored sequence number current, so a resume asks for the right events.
    fn advance(&self, seq: i64) {
        if let Ok(mut store) = self.resume.lock()
            && let Some(state) = store.get_mut(&self.login_id)
        {
            state.seq = Some(seq);
        }
    }

    fn forget(&self) {
        if let Ok(mut store) = self.resume.lock() {
            store.remove(&self.login_id);
        }
    }

    /// Drive one socket until it ends.
    async fn drive<S>(&mut self, mut socket: S, resume: Option<ResumeState>) -> Ending
    where
        S: Stream<Item = Result<WsMessage, WsError>> + Sink<WsMessage> + Unpin,
    {
        let mut heartbeat: Option<Heartbeat> = None;
        let mut identified = false;
        loop {
            let deadline = heartbeat.as_ref().map(|beat| beat.next);
            tokio::select! {
                message = socket.next() => {
                    let frame = match message {
                        Some(Ok(frame)) => frame,
                        Some(Err(err)) => {
                            debug!("gateway socket error: {err}");
                            return Ending::Transient;
                        }
                        None => return Ending::Transient,
                    };
                    let text = match frame {
                        WsMessage::Text(text) => text,
                        WsMessage::Close(frame) => {
                            return close_ending(frame.map(|frame| u16::from(frame.code)));
                        }
                        WsMessage::Ping(payload) => {
                            let _ = socket.send(WsMessage::Pong(payload)).await;
                            continue;
                        }
                        _ => continue,
                    };
                    let Ok(payload) = serde_json::from_str::<Value>(&text) else {
                        warn!("gateway sent a non-JSON frame");
                        continue;
                    };
                    if let Some(seq) = payload.get("s").and_then(Value::as_i64) {
                        self.sequence = Some(seq);
                        self.advance(seq);
                    }
                    let op = payload.get("op").and_then(Value::as_i64).unwrap_or(-1);
                    match op {
                        10 => {
                            let ms = payload
                                .pointer("/d/heartbeat_interval")
                                .and_then(Value::as_u64)
                                .filter(|ms| *ms > 0)
                                .unwrap_or(DEFAULT_HEARTBEAT_MS);
                            let interval = Duration::from_millis(ms);
                            // The first beat comes after a random share of the interval.
                            heartbeat = Some(Heartbeat {
                                interval,
                                next: Instant::now() + interval.mul_f64(jitter()),
                                acked: true,
                            });
                            if !identified {
                                let packet = match &resume {
                                    Some(state) => resume_packet(&self.token, state),
                                    None => identify_packet(&self.token, self.intents),
                                };
                                if socket
                                    .send(WsMessage::Text(packet.to_string().into()))
                                    .await
                                    .is_err()
                                {
                                    return Ending::Transient;
                                }
                                identified = true;
                            }
                        }
                        11 => {
                            if let Some(beat) = heartbeat.as_mut() {
                                beat.acked = true;
                            }
                        }
                        1 => send_heartbeat(&mut socket, self.sequence).await,
                        7 => {
                            debug!("gateway asked to reconnect");
                            return Ending::Transient;
                        }
                        9 => {
                            let resumable = payload.get("d").and_then(Value::as_bool).unwrap_or(false);
                            if !resumable {
                                self.forget();
                            }
                            debug!("gateway invalidated the session (resumable: {resumable})");
                            tokio::time::sleep(random_delay()).await;
                            return Ending::Transient;
                        }
                        0 => {
                            let kind = payload.get("t").and_then(Value::as_str).unwrap_or_default();
                            let data = payload.get("d").cloned().unwrap_or(Value::Null);
                            self.dispatch(kind, data);
                        }
                        _ => {}
                    }
                }
                () = beat(deadline) => {
                    if let Some(beat) = heartbeat.as_mut() {
                        if !beat.acked {
                            // Discord stopped answering. Close, keep the session for a resume.
                            let frame = CloseFrame {
                                code: CloseCode::from(4000),
                                reason: "".into(),
                            };
                            let _ = socket.send(WsMessage::Close(Some(frame))).await;
                            let _ = SinkExt::close(&mut socket).await;
                            return Ending::Transient;
                        }
                        beat.acked = false;
                        beat.next = Instant::now() + beat.interval;
                    }
                    send_heartbeat(&mut socket, self.sequence).await;
                }
            }
        }
    }

    /// Handle one dispatch event.
    fn dispatch(&self, kind: &str, data: Value) {
        match kind {
            "READY" => self.ready(&data),
            "RESUMED" => self.state(SessionState::Running, String::new()),
            "GUILD_CREATE" => {
                if let Ok(guild) = serde_json::from_value::<Guild>(data)
                    && !guild.unavailable
                {
                    self.guild(&guild);
                }
            }
            "MESSAGE_CREATE" => self.message_created(data),
            "MESSAGE_UPDATE" => self.message_updated(data),
            "MESSAGE_DELETE" => {
                let channel = text_field(&data, "channel_id");
                let id = text_field(&data, "id");
                if !channel.is_empty() && !id.is_empty() {
                    self.emit(Event::Redact {
                        portal: portal(channel),
                        event: RemoteEventRef::new(id),
                    });
                }
            }
            "MESSAGE_REACTION_ADD" | "MESSAGE_REACTION_REMOVE" => {
                self.reaction_changed(kind == "MESSAGE_REACTION_ADD", data);
            }
            "MESSAGE_REACTION_REMOVE_ALL" => self.reactions_cleared(data),
            "MESSAGE_REACTION_REMOVE_EMOJI" => self.emoji_cleared(data),
            "TYPING_START" => self.typing(&data),
            "CHANNEL_CREATE" | "CHANNEL_UPDATE" | "THREAD_CREATE" | "THREAD_UPDATE" => {
                if let Ok(channel) = serde_json::from_value::<Channel>(data) {
                    self.chat(&channel, "");
                }
            }
            "CHANNEL_DELETE" | "THREAD_DELETE" => {
                if let Ok(channel) = serde_json::from_value::<Channel>(data) {
                    self.emit(Event::ChatRemoved(portal(channel.id)));
                }
            }
            "THREAD_LIST_SYNC" => {
                for thread in parse_each::<Channel>(data.get("threads")) {
                    self.chat(&thread, "");
                }
            }
            "GUILD_MEMBER_ADD" | "GUILD_MEMBER_REMOVE" | "GUILD_MEMBER_UPDATE" => {
                self.member(kind, &data);
            }
            _ => {}
        }
    }

    fn ready(&self, data: &Value) {
        let session_id = data.get("session_id").and_then(Value::as_str);
        let resume_url = data.get("resume_gateway_url").and_then(Value::as_str);
        if let (Some(session_id), Some(resume_url)) = (session_id, resume_url) {
            self.store_resume(ResumeState {
                session_id: session_id.to_owned(),
                resume_gateway_url: resume_url.to_owned(),
                seq: self.sequence,
            });
        }
        let profile = data
            .get("user")
            .cloned()
            .and_then(|value| serde_json::from_value::<User>(value).ok())
            .map(|user| map::to_remote_user(&user));
        self.emit(Event::State {
            state: SessionState::Running,
            error: String::new(),
            message: String::new(),
            profile,
        });

        for guild in parse_each::<Guild>(data.get("guilds")) {
            if !guild.unavailable {
                self.guild(&guild);
            }
        }
        // User accounts name their DM partners by id; the users array resolves them.
        let users: Vec<User> = parse_each(data.get("users"));
        for mut channel in parse_each::<Channel>(data.get("private_channels")) {
            map::resolve_recipients(&mut channel, &users);
            self.chat(&channel, "");
        }
    }

    /// A guild as a space, with its channels and threads. Without a `channels` key the channels
    /// are read over REST.
    fn guild(&self, guild: &Guild) {
        self.emit(Event::Chat(map::guild_meta(guild)));
        match &guild.channels {
            Some(channels) => {
                for channel in channels {
                    self.chat(channel, &guild.name);
                }
            }
            None => self.spawn_guild_channels(guild.id.clone(), guild.name.clone()),
        }
        for thread in &guild.threads {
            self.chat(thread, &guild.name);
        }
    }

    fn spawn_guild_channels(&self, guild_id: String, guild_name: String) {
        let api = self.api.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            match api.guild_channels(&guild_id).await {
                Ok(channels) => {
                    for channel in channels {
                        if let Some(meta) = map::chat_meta(&channel, &guild_name) {
                            let _ = tx.send(Event::Chat(meta));
                        }
                    }
                }
                Err(err) => warn!("cannot read the channels of {guild_id}: {err}"),
            }
        });
    }

    fn chat(&self, channel: &Channel, guild_name: &str) {
        if let Some(meta) = map::chat_meta(channel, guild_name) {
            self.emit(Event::Chat(meta));
        }
    }

    fn message_created(&self, data: Value) {
        let has_reactions = data.get("reactions").is_some();
        let Ok(message) = serde_json::from_value::<Message>(data) else {
            return;
        };
        if map::is_system(&message) {
            return;
        }
        if has_reactions {
            self.refresh_reactions(&message);
        }
        self.spawn_message(message);
    }

    /// An edit is only an edit when it has new text and an edit timestamp. Partial updates, such
    /// as embed unfurls, are ignored.
    fn message_updated(&self, data: Value) {
        let has_reactions = data.get("reactions").is_some();
        let Ok(update) = serde_json::from_value::<MessageUpdate>(data) else {
            return;
        };
        if !update.is_edit() {
            return;
        }
        if let Some(message) = update.as_message() {
            if has_reactions {
                self.refresh_reactions(&message);
            }
            self.spawn_message(message);
            return;
        }
        // An edit without its author: read the whole message once.
        let (Some(channel), Some(id)) = (update.channel_id, update.id) else {
            return;
        };
        let api = self.api.clone();
        let tx = self.tx.clone();
        let me_id = self.me_id.clone();
        tokio::spawn(async move {
            match api.message(&channel, &id).await {
                Ok(message) => {
                    let remote = map::to_remote_message(&api, &message, true, &me_id).await;
                    let _ = tx.send(Event::Message(remote));
                }
                Err(err) => debug!("discord: cannot read an edit: {err}"),
            }
        });
    }

    fn spawn_message(&self, message: Message) {
        let api = self.api.clone();
        let tx = self.tx.clone();
        let me_id = self.me_id.clone();
        tokio::spawn(async move {
            let remote = map::to_remote_message(&api, &message, true, &me_id).await;
            let _ = tx.send(Event::Message(remote));
        });
    }

    /// Keep the summary of a cached message in step with the payload that carries it.
    fn refresh_reactions(&self, message: &Message) {
        let key = (message.channel_id.clone(), message.id.clone());
        if let Ok(mut cache) = self.reactions.lock() {
            let reactions = map::reactions(message);
            cache.change(&key, |summary| *summary = reactions);
        }
    }

    fn reaction_changed(&self, added: bool, data: Value) {
        let Ok(event) = serde_json::from_value::<ReactionEvent>(data) else {
            return;
        };
        if event.channel_id.is_empty() || event.message_id.is_empty() {
            return;
        }
        let key = map::emoji_key(&event.emoji);
        let delta = if added { 1 } else { -1 };
        let from_me = event.user_id.as_deref() == Some(self.me_id.as_str());
        let cache_key = (event.channel_id.clone(), event.message_id.clone());
        let updated = self.reactions.lock().ok().and_then(|mut cache| {
            cache.change(&cache_key, |summary| {
                apply_delta(summary, &key, delta, from_me);
            })
        });
        match updated {
            Some(reactions) => self.emit_reactions(event.channel_id, event.message_id, reactions),
            None => self.fetch_reactions(event.channel_id, event.message_id),
        }
    }

    fn reactions_cleared(&self, data: Value) {
        let Ok(event) = serde_json::from_value::<ReactionEvent>(data) else {
            return;
        };
        if event.channel_id.is_empty() || event.message_id.is_empty() {
            return;
        }
        let key = (event.channel_id.clone(), event.message_id.clone());
        if let Ok(mut cache) = self.reactions.lock() {
            cache.store(key, Vec::new());
        }
        self.emit_reactions(event.channel_id, event.message_id, Vec::new());
    }

    fn emoji_cleared(&self, data: Value) {
        let Ok(event) = serde_json::from_value::<ReactionEvent>(data) else {
            return;
        };
        if event.channel_id.is_empty() || event.message_id.is_empty() {
            return;
        }
        let key = map::emoji_key(&event.emoji);
        let cache_key = (event.channel_id.clone(), event.message_id.clone());
        let updated = self.reactions.lock().ok().and_then(|mut cache| {
            cache.change(&cache_key, |summary| {
                summary.retain(|reaction| reaction.key != key);
            })
        });
        match updated {
            Some(reactions) => self.emit_reactions(event.channel_id, event.message_id, reactions),
            None => self.fetch_reactions(event.channel_id, event.message_id),
        }
    }

    fn emit_reactions(&self, channel: String, message: String, reactions: Vec<Reaction>) {
        self.emit(Event::Reactions {
            portal: portal(channel),
            target: RemoteEventRef::new(message),
            reactions,
        });
    }

    /// A message that is not cached: read it once, cache its summary and report it.
    fn fetch_reactions(&self, channel: String, message: String) {
        let api = self.api.clone();
        let tx = self.tx.clone();
        let cache = self.reactions.clone();
        tokio::spawn(async move {
            // Discord reports one reactor per event; the UI wants the summary, so read it.
            let full = match api.message(&channel, &message).await {
                Ok(full) => full,
                Err(err) => {
                    debug!("discord: cannot read the reactions of a message: {err}");
                    return;
                }
            };
            let reactions = map::reactions(&full);
            if let Ok(mut cache) = cache.lock() {
                cache.store((channel.clone(), message.clone()), reactions.clone());
            }
            let _ = tx.send(Event::Reactions {
                portal: portal(channel),
                target: RemoteEventRef::new(message),
                reactions,
            });
        });
    }

    fn typing(&self, data: &Value) {
        let channel = text_field(data, "channel_id");
        if channel.is_empty() {
            return;
        }
        let user = data
            .pointer("/member/user")
            .cloned()
            .and_then(|value| serde_json::from_value::<User>(value).ok())
            .or_else(|| {
                data.get("user_id").and_then(Value::as_str).map(|id| User {
                    id: id.to_owned(),
                    ..Default::default()
                })
            });
        if let Some(user) = user {
            self.emit(Event::Typing {
                portal: portal(channel),
                senders: vec![map::to_remote_user(&user)],
            });
        }
    }

    fn member(&self, kind: &str, data: &Value) {
        let guild = text_field(data, "guild_id");
        if guild.is_empty() {
            return;
        }
        let Some(user) = data
            .get("user")
            .cloned()
            .and_then(|value| serde_json::from_value::<User>(value).ok())
        else {
            return;
        };
        let membership = match kind {
            "GUILD_MEMBER_REMOVE" => "leave",
            _ => "join",
        };
        self.emit(Event::Member {
            portal: portal(guild),
            member: Member {
                user: map::to_remote_user(&user),
                membership: membership.to_owned(),
            },
        });
    }
}

/// The session outcome of a close code.
fn close_ending(code: Option<u16>) -> Ending {
    match code {
        Some(4004) => Ending::Stop {
            state: SessionState::BadCredentials,
            error: "Discord refused the token".to_owned(),
        },
        // An invalid sequence or an expired session cannot be resumed; the next connect identifies.
        Some(4007 | 4009) => Ending::Transient,
        Some(4010) => unknown_close("Discord rejected the gateway shard setting (code 4010)."),
        Some(4011) => unknown_close("Discord requires sharding for this bot (code 4011)."),
        Some(4012) => unknown_close("Discord rejected the gateway API version (code 4012)."),
        Some(4013) => unknown_close("Discord rejected the requested gateway intents (code 4013)."),
        Some(4014) => unknown_close(
            "Discord refused a privileged intent. Enable the Message Content intent for this bot in the Discord developer portal.",
        ),
        _ => Ending::Transient,
    }
}

fn unknown_close(text: &str) -> Ending {
    Ending::Stop {
        state: SessionState::UnknownError,
        error: text.to_owned(),
    }
}

fn identify_packet(token: &str, intents: u64) -> Value {
    json!({
        "op": 2,
        "d": {
            "token": token,
            "intents": intents,
            "properties": {
                "os": std::env::consts::OS,
                "browser": "Chirper",
                "device": "Chirper"
            },
            "compress": false
        }
    })
}

fn resume_packet(token: &str, state: &ResumeState) -> Value {
    json!({
        "op": 6,
        "d": {
            "token": token,
            "session_id": state.session_id,
            "seq": state.seq
        }
    })
}

async fn send_heartbeat<S>(socket: &mut S, sequence: Option<i64>)
where
    S: SinkExt<WsMessage> + Unpin,
{
    let payload = json!({ "op": 1, "d": sequence });
    let _ = socket
        .send(WsMessage::Text(payload.to_string().into()))
        .await;
}

/// Wait until the heartbeat is due; never resolves when there is no heartbeat yet.
async fn beat(deadline: Option<Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// A uniform value in [0, 1) from the OS random source; 0.5 when the source is unavailable.
fn jitter() -> f64 {
    let mut bytes = [0u8; 4];
    match getrandom::fill(&mut bytes) {
        Ok(()) => f64::from(u32::from_le_bytes(bytes)) / (f64::from(u32::MAX) + 1.0),
        Err(_) => 0.5,
    }
}

/// A pause of one to five seconds before reconnecting after an invalid session.
fn random_delay() -> Duration {
    Duration::from_millis(1_000 + (jitter() * 4_000.0) as u64)
}

/// Read a string field, or the empty string.
fn text_field<'a>(data: &'a Value, key: &str) -> &'a str {
    data.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// Parse each element of an array on its own, skipping the ones that do not parse.
fn parse_each<T: DeserializeOwned>(value: Option<&Value>) -> Vec<T> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| serde_json::from_value(item.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn portal(channel: impl Into<String>) -> PortalRef {
    PortalRef {
        id: channel.into(),
        receiver: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reaction(key: &str, count: i64, from_me: bool) -> Reaction {
        Reaction {
            key: key.to_owned(),
            count,
            from_me,
            senders: Vec::new(),
        }
    }

    #[test]
    fn a_new_reaction_starts_at_one() {
        let mut summary = Vec::new();
        apply_delta(&mut summary, "👍", 1, false);
        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0].count, 1);
        assert!(!summary[0].from_me);
    }

    #[test]
    fn adding_and_removing_moves_the_count() {
        let mut summary = vec![reaction("👍", 2, false)];
        apply_delta(&mut summary, "👍", 1, false);
        assert_eq!(summary[0].count, 3);
        apply_delta(&mut summary, "👍", -1, false);
        apply_delta(&mut summary, "👍", -1, false);
        assert_eq!(summary[0].count, 1);
    }

    #[test]
    fn an_emoji_drops_out_at_zero() {
        let mut summary = vec![reaction("wave:1", 1, false), reaction("👍", 4, false)];
        apply_delta(&mut summary, "wave:1", -1, false);
        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0].key, "👍");
    }

    #[test]
    fn from_me_follows_the_local_users_events() {
        let mut summary = Vec::new();
        apply_delta(&mut summary, "🎉", 1, true);
        assert!(summary[0].from_me);
        apply_delta(&mut summary, "🎉", -1, true);
        assert!(summary.is_empty());

        let mut other = vec![reaction("🎉", 1, false)];
        apply_delta(&mut other, "🎉", 1, false);
        assert!(
            !other[0].from_me,
            "someone else's reaction does not change from_me"
        );
    }

    #[test]
    fn a_removal_of_an_unknown_emoji_adds_nothing() {
        let mut summary = Vec::new();
        apply_delta(&mut summary, "🎉", -1, false);
        assert!(summary.is_empty());
    }

    #[test]
    fn the_cache_only_changes_messages_it_holds() {
        let mut cache = ReactionCache::default();
        let key = ("1".to_owned(), "2".to_owned());
        assert!(cache.change(&key, |_| {}).is_none());
        cache.store(key.clone(), vec![reaction("👍", 1, false)]);
        let changed = cache.change(&key, |summary| apply_delta(summary, "👍", 1, false));
        assert_eq!(changed.map(|s| s[0].count), Some(2));
    }

    #[test]
    fn the_cache_evicts_the_oldest_entries() {
        let mut cache = ReactionCache::default();
        for index in 0..(REACTION_CACHE_LIMIT + 10) {
            cache.store(("c".to_owned(), index.to_string()), Vec::new());
        }
        assert_eq!(cache.entries.len(), REACTION_CACHE_LIMIT);
        assert!(
            !cache
                .entries
                .contains_key(&("c".to_owned(), "0".to_owned()))
        );
        assert!(
            cache
                .entries
                .contains_key(&("c".to_owned(), (REACTION_CACHE_LIMIT + 9).to_string()))
        );
    }

    #[test]
    fn close_codes_map_to_session_outcomes() {
        assert_eq!(
            close_ending(Some(4004)),
            Ending::Stop {
                state: SessionState::BadCredentials,
                error: "Discord refused the token".to_owned(),
            }
        );
        assert_eq!(close_ending(Some(4007)), Ending::Transient);
        assert_eq!(close_ending(Some(4009)), Ending::Transient);
        assert_eq!(close_ending(None), Ending::Transient);
        assert_eq!(close_ending(Some(1000)), Ending::Transient);
        match close_ending(Some(4014)) {
            Ending::Stop { state, error } => {
                assert_eq!(state, SessionState::UnknownError);
                assert!(error.contains("Message Content intent"));
            }
            other => panic!("expected a stop, got {other:?}"),
        }
    }

    #[test]
    fn intents_keep_the_reaction_and_typing_bits() {
        assert_ne!(USER_INTENTS & (1 << 10), 0, "GUILD_MESSAGE_REACTIONS");
        assert_ne!(USER_INTENTS & (1 << 11), 0, "GUILD_MESSAGE_TYPING");
        assert_ne!(USER_INTENTS & (1 << 15), 0, "MESSAGE_CONTENT");
    }

    #[test]
    fn bots_identify_with_the_go_connectors_intents() {
        assert_ne!(BOT_INTENTS & (1 << 1), 0, "GUILD_MEMBERS");
        assert_eq!(BOT_INTENTS & !(1 << 1), USER_INTENTS);
        let bot = identify_packet("Bot tok", BOT_INTENTS);
        assert_eq!(bot["op"], 2);
        assert_eq!(bot["d"]["token"], "Bot tok");
        assert_eq!(bot["d"]["intents"], 65027);
        assert_eq!(identify_packet("tok", USER_INTENTS)["d"]["intents"], 65025);
    }

    #[test]
    fn resume_packets_carry_the_session() {
        let state = ResumeState {
            session_id: "abc".to_owned(),
            resume_gateway_url: "wss://gateway.example".to_owned(),
            seq: Some(7),
        };
        let packet = resume_packet("tok", &state);
        assert_eq!(packet["op"], 6);
        assert_eq!(packet["d"]["session_id"], "abc");
        assert_eq!(packet["d"]["seq"], 7);
    }

    #[test]
    fn random_values_stay_in_range() {
        for _ in 0..100 {
            let value = jitter();
            assert!((0.0..1.0).contains(&value));
            let delay = random_delay();
            assert!(delay >= Duration::from_secs(1) && delay < Duration::from_secs(5));
        }
    }
}
