//! One live connection: the start-up sequence, the read loop with keep-alive, request/response
//! plumbing and the mapping of bridge commands onto protocol messages.
//!
//! The session is generic over [`Wire`] so tests can run the whole path against an in-memory
//! server. Everything the network says becomes a queue of bridge [`Event`]s, drained by
//! [`Session::next`]. Nothing is awaited inside `next` except the socket, so it is cancel-safe.

use std::collections::{HashMap, HashSet, VecDeque};

use async_trait::async_trait;
use chirper_bridge::model::{
    Ack, BackfillPage, Capabilities, ChatMeta, Content, DeliveryStatus, PortalRef, RemoteUser,
    SessionState,
};
use chirper_bridge::{Command, Error, Event, LoginCtx, Result, Session};
use rand::rngs::OsRng;
use tokio::time::{Duration, Instant};

use crate::binary::{Jid, Node};
use crate::keys::DeviceKeys;
use crate::media::MediaConn;
use crate::store::SignalStore;
use crate::transport::Wire;
use crate::{ids, pair, proto};

/// The longest a request waits for its response.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Keep-alive interval bounds, as whatsmeow's `keepalive.go`.
const KEEPALIVE_MIN: Duration = Duration::from_secs(20);
const KEEPALIVE_MAX: Duration = Duration::from_secs(30);

/// How long a keep-alive may go unanswered before the connection is declared dead.
const KEEPALIVE_DEADLINE: Duration = Duration::from_secs(10);

/// Prekeys below this count on the server trigger an upload.
pub const MIN_PRE_KEY_COUNT: u32 = 5;

/// Prekeys uploaded when the server has none, and when it has some.
pub const INITIAL_PRE_KEY_UPLOAD: u32 = 812;
pub const WANTED_PRE_KEY_UPLOAD: u32 = 50;

/// The most group chats the start-up sync reports.
pub const MAX_CHATS: usize = 500;

/// The server's JID, the target of every server request.
pub const SERVER_JID: &str = "s.whatsapp.net";

/// The keep-alive state: when the next ping goes out, and the one in flight.
#[derive(Debug)]
pub struct Keepalive {
    pub next: Instant,
    pub pending: Option<(String, Instant)>,
}

/// Reactions per message: one reaction per reactor, as WhatsApp sends them.
#[derive(Default)]
pub struct Reactions {
    /// message id -> (reactor user id, emoji)
    by_message: HashMap<String, Vec<(String, String)>>,
}

impl Reactions {
    /// The bound on messages with remembered reactions.
    const MAX_MESSAGES: usize = 4096;

    /// Records one reactor's emoji on a message (an empty emoji removes it) and returns the full
    /// set of reactions the message now has.
    pub fn set(&mut self, message: &str, reactor: &str, emoji: &str) -> Vec<(String, String)> {
        if !self.by_message.contains_key(message) && self.by_message.len() >= Self::MAX_MESSAGES {
            self.by_message.clear();
        }
        let entry = self.by_message.entry(message.to_owned()).or_default();
        entry.retain(|(who, _)| who != reactor);
        if !emoji.is_empty() {
            entry.push((reactor.to_owned(), emoji.to_owned()));
        }
        entry.clone()
    }
}

/// A live WhatsApp connection of one login.
pub struct Client<W: Wire> {
    pub(crate) wire: W,
    pub(crate) ctx: LoginCtx,
    pub(crate) keys: DeviceKeys,
    /// This device's JID.
    pub(crate) own: Jid,
    pub(crate) signal: SignalStore,
    pub(crate) rng: OsRng,
    pub(crate) events: VecDeque<Event>,
    /// Nodes that arrived while a request was waiting for its own response. Handled in order later.
    pub(crate) pending: VecDeque<Node>,
    pub(crate) counter: u64,
    pub(crate) keepalive: Keepalive,
    pub(crate) media_conn: Option<MediaConn>,
    /// The devices of each user, from usync. Invalidated by `devices` notifications.
    pub(crate) devices: HashMap<String, Vec<Jid>>,
    /// The members of each group, from the group query.
    pub(crate) members: HashMap<String, Vec<Jid>>,
    pub(crate) reactions: Reactions,
    /// Display names from push names and contact actions, by user id.
    pub(crate) names: HashMap<String, String>,
    /// The last incoming message per chat, for a read receipt without an explicit target.
    pub(crate) last_seen: HashMap<String, String>,
    /// Retry counts of messages we could not decrypt.
    pub(crate) retries: HashMap<String, u32>,
    /// Chats whose on-demand history was requested this session.
    pub(crate) requested_history: HashSet<String>,
    /// The base of media URLs; `None` means `https://<host>`. Tests point it at a local server.
    pub(crate) media_base: Option<String>,
    pub(crate) closed: bool,
    /// Chats announced to the bridge this session, so each is announced once.
    pub(crate) seen_chats: HashSet<String>,
    /// The own push name, when the account has one.
    pub(crate) push_name: String,
}

impl<W: Wire> std::fmt::Debug for Client<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("closed", &self.closed)
            .finish_non_exhaustive()
    }
}

impl<W: Wire> Client<W> {
    /// Brings a connection that carries the login payload to `Running`: waits for `success`,
    /// keeps the prekeys topped up, turns the passive flag off, reports the chats and syncs the
    /// app state. The returned client has the events of all of that queued.
    pub async fn start(
        wire: W,
        ctx: LoginCtx,
        keys: DeviceKeys,
        media_base: Option<String>,
    ) -> Result<Self> {
        let own = keys
            .jid
            .clone()
            .ok_or_else(|| Error::Auth("the device record has no account".to_owned()))?;
        let signal = SignalStore::new(ctx.clone(), &keys)?;
        let mut client = Self {
            wire,
            ctx,
            keys,
            own: own.clone(),
            signal,
            rng: OsRng,
            events: VecDeque::new(),
            pending: VecDeque::new(),
            counter: 0,
            keepalive: Keepalive {
                next: Instant::now() + KEEPALIVE_MIN,
                pending: None,
            },
            media_conn: None,
            devices: HashMap::new(),
            members: HashMap::new(),
            reactions: Reactions::default(),
            names: HashMap::new(),
            last_seen: HashMap::new(),
            retries: HashMap::new(),
            requested_history: HashSet::new(),
            media_base,
            closed: false,
            seen_chats: HashSet::new(),
            push_name: String::new(),
        };
        client.events.push_back(Event::State {
            state: SessionState::Connecting,
            error: String::new(),
            message: String::new(),
            profile: None,
        });
        client.await_success().await?;
        client.ensure_pre_keys().await?;
        client.set_passive(false).await?;
        client.sync_chats().await?;
        client.sync_app_state().await;
        let profile = client.profile();
        client.events.push_back(Event::State {
            state: SessionState::Running,
            error: String::new(),
            message: String::new(),
            profile: Some(profile),
        });
        Ok(client)
    }

    /// Waits for the server's `success`, answering whatever else it asks in the meantime.
    async fn await_success(&mut self) -> Result<()> {
        tokio::time::timeout(REQUEST_TIMEOUT, async {
            loop {
                let node = self.wire.next_node().await?;
                match node.tag.as_str() {
                    "success" => return Ok(()),
                    _ => self.pending.push_back(node),
                }
            }
        })
        .await
        .map_err(|_| Error::Network("the server did not accept the login in time".to_owned()))??;
        Ok(())
    }

    /// The next id for a request.
    pub(crate) fn next_id(&mut self) -> String {
        self.counter += 1;
        format!("chirper.{}", self.counter)
    }

    /// Sends a node to the server.
    pub(crate) async fn send(&mut self, node: &Node) -> Result<()> {
        self.wire.send_node(node).await
    }

    /// Sends an `iq` and waits for its result. Other nodes that arrive meanwhile are handled.
    pub(crate) async fn request(&mut self, iq: Node) -> Result<Node> {
        let id = iq
            .get_attr("id")
            .ok_or_else(|| Error::Internal("a request without an id".to_owned()))?;
        self.send(&iq).await?;
        self.await_response(&id).await
    }

    /// Reads until the response to `id` arrives.
    pub(crate) async fn await_response(&mut self, id: &str) -> Result<Node> {
        tokio::time::timeout(REQUEST_TIMEOUT, async {
            loop {
                let node = self.wire.next_node().await?;
                if node.tag == "iq" && node.get_attr("id").as_deref() == Some(id) {
                    return match node.get_attr("type").as_deref() {
                        Some("error") => Err(iq_error(&node)),
                        _ => Ok(node),
                    };
                }
                self.pending.push_back(node);
            }
        })
        .await
        .map_err(|_| Error::Network("the server did not answer in time".to_owned()))?
    }

    /// Reads until the server's `ack` for message `id` arrives.
    pub(crate) async fn await_message_ack(&mut self, id: &str) -> Result<()> {
        tokio::time::timeout(REQUEST_TIMEOUT, async {
            loop {
                let node = self.wire.next_node().await?;
                if node.tag == "ack" && node.get_attr("id").as_deref() == Some(id) {
                    return match node.get_attr("error") {
                        Some(code) => Err(Error::Protocol(format!(
                            "the server refused the message ({code})"
                        ))),
                        None => Ok(()),
                    };
                }
                self.pending.push_back(node);
            }
        })
        .await
        .map_err(|_| Error::Network("the server did not confirm the message in time".to_owned()))?
    }

    /// The `ack` of a node we received: its class, id and addressing echoed back.
    pub(crate) fn ack_for(node: &Node, error: Option<u32>) -> Node {
        let mut ack = Node::new("ack")
            .attr("class", node.tag.clone())
            .attr("id", node.get_attr("id").unwrap_or_default());
        if let Some(to) = node.get_attr("from") {
            ack = ack.attr("to", to);
        }
        if let Some(participant) = node.get_attr("participant") {
            ack = ack.attr("participant", participant);
        }
        if let Some(recipient) = node.get_attr("recipient") {
            ack = ack.attr("recipient", recipient);
        }
        if node.tag != "message"
            && let Some(kind) = node.get_attr("type")
        {
            ack = ack.attr("type", kind);
        }
        if let Some(code) = error {
            ack = ack.attr("error", code.to_string());
        }
        ack
    }

    /// Handles one server node that is not the response to a request we are waiting for.
    pub(crate) async fn handle_node(&mut self, node: Node) -> Result<()> {
        if self.is_keepalive_response(&node) {
            self.keepalive.pending = None;
            return Ok(());
        }
        match node.tag.as_str() {
            "message" => self.on_message(&node).await,
            "receipt" => self.on_receipt(&node).await,
            "chatstate" => self.on_chatstate(&node).await,
            "notification" => self.on_notification(&node).await,
            "call" => self.on_call(&node).await,
            "ack" => Ok(()),
            "iq" => {
                if let Some(ack) = pair::ack(&node) {
                    self.send(&ack).await?;
                }
                Ok(())
            }
            "success" | "presence" | "ib" | "appdata" => Ok(()),
            "failure" => Err(self.failure_error(&node)),
            "stream:error" => Err(self.stream_error(&node)),
            other => {
                log::debug!("ignoring a {other} node");
                Ok(())
            }
        }
    }

    fn is_keepalive_response(&self, node: &Node) -> bool {
        node.tag == "iq"
            && self
                .keepalive
                .pending
                .as_ref()
                .is_some_and(|(id, _)| node.get_attr("id").as_deref() == Some(id.as_str()))
    }

    fn failure_error(&self, node: &Node) -> Error {
        match node.get_attr("reason").as_deref() {
            Some("401") | Some("403") => {
                Error::Auth("the server logged this device out".to_owned())
            }
            _ => Error::Network("the server refused the connection".to_owned()),
        }
    }

    fn stream_error(&self, node: &Node) -> Error {
        let code = node.get_attr("code").unwrap_or_default();
        let conflict = node.child("conflict");
        if code == "401"
            || conflict.is_some_and(|conflict| {
                conflict.get_attr("type").as_deref() == Some("device_removed")
            })
        {
            return Error::Auth("the server removed this device".to_owned());
        }
        if conflict.is_some() {
            return Error::Auth("another device took over this connection".to_owned());
        }
        Error::Network(format!("the server ended the connection ({code})"))
    }

    /// Reads the next thing from the server, a keep-alive or a deadline, and queues its events.
    async fn step(&mut self) -> Result<()> {
        if let Some(node) = self.pending.pop_front() {
            return self.handle_node(node).await;
        }
        let wake = match &self.keepalive.pending {
            Some((_, deadline)) => *deadline,
            None => self.keepalive.next,
        };
        let node = tokio::select! {
            node = self.wire.next_node() => node?,
            _ = tokio::time::sleep_until(wake) => {
                return self.keepalive_tick().await;
            }
        };
        self.handle_node(node).await
    }

    async fn keepalive_tick(&mut self) -> Result<()> {
        if let Some((_, deadline)) = &self.keepalive.pending
            && Instant::now() >= *deadline
        {
            return Err(Error::Network("the keep-alive went unanswered".to_owned()));
        }
        if self.keepalive.pending.is_none() {
            let id = self.next_id();
            let ping = Node::new("iq")
                .attr("to", SERVER_JID)
                .attr("type", "get")
                .attr("xmlns", "w:p")
                .attr("id", id.clone());
            self.send(&ping).await?;
            self.keepalive.pending = Some((id, Instant::now() + KEEPALIVE_DEADLINE));
            self.keepalive.next = Instant::now() + pick_interval(self.counter);
        }
        Ok(())
    }

    /// Asks the server for the prekey count, and uploads a batch when it is low.
    pub(crate) async fn ensure_pre_keys(&mut self) -> Result<()> {
        let id = self.next_id();
        let count_request = Node::new("iq")
            .attr("to", SERVER_JID)
            .attr("type", "get")
            .attr("xmlns", "encrypt")
            .attr("id", id)
            .children(vec![Node::new("count")]);
        let reply = self.request(count_request).await?;
        let server_count = reply
            .child("count")
            .and_then(|count| count.get_attr("value"))
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);
        if server_count < MIN_PRE_KEY_COUNT {
            let batch = if server_count == 0 {
                INITIAL_PRE_KEY_UPLOAD
            } else {
                WANTED_PRE_KEY_UPLOAD
            };
            self.upload_pre_keys(batch).await?;
        }
        Ok(())
    }

    /// Generates `count` one-time prekeys, stores them and uploads them with the signed prekey.
    pub(crate) async fn upload_pre_keys(&mut self, count: u32) -> Result<()> {
        let start = self.next_pre_key_id()?;
        let keys = chirper_signal_protocol::prekey::generate_pre_keys(start, count, &mut self.rng)
            .map_err(|_| Error::Internal("prekeys could not be generated".to_owned()))?;
        for record in &keys {
            use chirper_signal_protocol::store::PreKeyStore;
            self.signal
                .store_pre_key(record)
                .map_err(|_| Error::Internal("a prekey could not be stored".to_owned()))?;
        }
        self.ctx.set_secret(
            "prekey-next",
            (start.wrapping_add(count)).to_string().as_bytes(),
        )?;
        let id = self.next_id();
        let signed = self.signal.signed_pre_key_record().clone();
        let request = Node::new("iq")
            .attr("to", SERVER_JID)
            .attr("type", "set")
            .attr("xmlns", "encrypt")
            .attr("id", id)
            .children(vec![
                Node::new("registration")
                    .bytes(self.signal.registration_id().to_be_bytes().to_vec()),
                Node::new("type").bytes(vec![0x05]),
                Node::new("identity").bytes(self.signal.identity_public().to_vec()),
                Node::new("list").children(keys.iter().map(pre_key_node).collect()),
                signed_pre_key_node(&signed),
            ]);
        self.request(request).await?;
        Ok(())
    }

    fn next_pre_key_id(&self) -> Result<u32> {
        let stored = self
            .ctx
            .get_secret("prekey-next")?
            .and_then(|bytes| String::from_utf8(bytes.to_vec()).ok())
            .and_then(|text| text.parse::<u32>().ok());
        Ok(stored.unwrap_or(1).max(1))
    }

    /// Sets whether the device receives events while the app is in the background.
    pub(crate) async fn set_passive(&mut self, passive: bool) -> Result<()> {
        let id = self.next_id();
        let tag = if passive { "passive" } else { "active" };
        let request = Node::new("iq")
            .attr("to", SERVER_JID)
            .attr("type", "set")
            .attr("xmlns", "passive")
            .attr("id", id)
            .children(vec![Node::new(tag)]);
        self.request(request).await?;
        Ok(())
    }

    /// The groups the account is in, as chats, each with its picture URL when it has one.
    pub(crate) async fn sync_chats(&mut self) -> Result<()> {
        let id = self.next_id();
        let request = Node::new("iq")
            .attr("to", "g.us")
            .attr("type", "get")
            .attr("xmlns", "w:g2")
            .attr("id", id)
            .children(vec![Node::new("participating").children(vec![
                Node::new("participants"),
                Node::new("description"),
            ])]);
        let reply = match self.request(request).await {
            Ok(reply) => reply,
            Err(error @ Error::Network(_)) => return Err(error),
            Err(error) => {
                log::debug!("the group list was refused: {error}");
                return Ok(());
            }
        };
        let groups = reply.child("participating").unwrap_or(&reply);
        let metas: Vec<ChatMeta> = groups
            .child_nodes()
            .iter()
            .filter(|node| node.tag == "group")
            .take(MAX_CHATS)
            .filter_map(|group| self.group_meta(group))
            .collect();
        for mut meta in metas {
            let jid: Option<Jid> = meta.portal.id.parse().ok();
            if let Some(jid) = jid {
                meta.avatar_url = self.avatar_url(&jid).await?;
            }
            self.events.push_back(Event::Chat(meta));
        }
        Ok(())
    }

    /// The picture URL of a chat or user, empty when it has none.
    pub(crate) async fn avatar_url(&mut self, jid: &Jid) -> Result<String> {
        let id = self.next_id();
        let request = Node::new("iq")
            .attr("to", jid.to_string())
            .attr("type", "get")
            .attr("xmlns", "w:profile:picture")
            .attr("id", id)
            .children(vec![
                Node::new("picture")
                    .attr("type", "preview")
                    .attr("query", "url"),
            ]);
        match self.request(request).await {
            Ok(reply) => Ok(reply
                .child("picture")
                .and_then(|picture| picture.get_attr("url"))
                .unwrap_or_default()),
            Err(error @ Error::Network(_)) => Err(error),
            Err(error) => {
                log::debug!("no picture for a chat: {error}");
                Ok(String::new())
            }
        }
    }

    /// The chat of a `group` node of the group query.
    pub(crate) fn group_meta(&mut self, group: &Node) -> Option<ChatMeta> {
        let id = group.get_attr("id")?;
        let jid: Jid = format!("{id}@g.us").parse().ok()?;
        let participants: Vec<Jid> = group
            .child_nodes()
            .iter()
            .filter(|node| node.tag == "participant")
            .filter_map(|node| node.jid_or_text("jid"))
            .collect();
        let member_count = i64::try_from(participants.len()).unwrap_or(i64::MAX);
        if !participants.is_empty() {
            self.members.insert(jid.to_string(), participants);
        }
        Some(ChatMeta {
            portal: self.portal(&jid),
            name: group.get_attr("subject").unwrap_or_default(),
            topic: group
                .child("description")
                .and_then(Node::content_bytes)
                .and_then(|bytes| String::from_utf8(bytes.to_vec()).ok())
                .unwrap_or_default(),
            member_count,
            ..Default::default()
        })
    }

    /// Downloads and imports the app state collections. A failure is logged, not fatal.
    pub(crate) async fn sync_app_state(&mut self) {
        for name in crate::appstate::COLLECTIONS {
            if let Err(error) = self.sync_collection(name).await {
                log::debug!("app state {name} did not sync: {error}");
            }
        }
    }

    /// The portal of a chat as this login sees it.
    pub(crate) fn portal(&self, chat: &Jid) -> PortalRef {
        PortalRef {
            id: ids::portal_id(chat),
            receiver: ids::portal_receiver(chat, &ids::user_login_id(&self.own)),
        }
    }

    /// This account's user JID, without the device.
    pub(crate) fn own_user(&self) -> Jid {
        Jid {
            device: 0,
            agent: 0,
            ..self.own.clone()
        }
    }

    /// The logged-in user as the bridge shows it.
    pub(crate) fn profile(&self) -> RemoteUser {
        RemoteUser {
            id: ids::user_id(&self.own),
            display_name: self.push_name.clone(),
            avatar_url: String::new(),
            username: String::new(),
            phone: self.own.user.clone(),
        }
    }

    /// The next event, reading from the server as needed. `None` when the connection ended.
    async fn next_event(&mut self) -> Option<Event> {
        loop {
            if let Some(event) = self.events.pop_front() {
                return Some(event);
            }
            if self.closed {
                return None;
            }
            if let Err(error) = self.step().await {
                self.closed = true;
                let state = error.session_state();
                log::debug!("the connection ended: {error}");
                return Some(Event::State {
                    state,
                    error: error.to_string(),
                    message: String::new(),
                    profile: None,
                });
            }
        }
    }

    /// Every bridge command, mapped onto protocol messages.
    pub(crate) async fn dispatch(&mut self, command: Command) -> Result<Ack> {
        match command {
            Command::SendText {
                portal,
                text,
                reply_to,
                txn_id,
            } => {
                let chat = self.chat_of(&portal)?;
                let message = crate::convert::text_message(&text, reply_to.as_deref());
                let id = self.send_new(&chat, message, "").await?;
                self.events.push_back(Event::Delivery {
                    portal,
                    txn_id,
                    status: DeliveryStatus::Sent,
                    error: String::new(),
                });
                Ok(self.ack_for_sent(&chat, &id))
            }
            Command::SendMedia {
                portal,
                data,
                mime,
                file_name,
                caption,
                kind,
                reply_to,
                txn_id,
            } => {
                let chat = self.chat_of(&portal)?;
                let message = self
                    .media_message(
                        &chat,
                        &data,
                        &mime,
                        &file_name,
                        &caption,
                        &kind,
                        reply_to.as_deref(),
                    )
                    .await?;
                let id = self.send_new(&chat, message, "").await?;
                self.events.push_back(Event::Delivery {
                    portal,
                    txn_id,
                    status: DeliveryStatus::Sent,
                    error: String::new(),
                });
                Ok(self.ack_for_sent(&chat, &id))
            }
            Command::Edit {
                portal,
                target,
                text,
            } => {
                let chat = self.chat_of(&portal)?;
                let target = crate::ids::parse_message_id(&target)
                    .ok_or(Error::Unsupported("editing that message"))?;
                let message = crate::convert::edit_message(&chat, &target, &text, &self.own);
                self.send_new(&chat, message, "1").await?;
                Ok(Ack::default())
            }
            Command::Redact { portal, target } => {
                let chat = self.chat_of(&portal)?;
                let target = crate::ids::parse_message_id(&target)
                    .ok_or(Error::Unsupported("deleting that message"))?;
                let from_me = target.sender.user == self.own.user;
                let edit = if from_me { "7" } else { "8" };
                let message = crate::convert::revoke_message(&chat, &target, from_me);
                self.send_new(&chat, message, edit).await?;
                Ok(Ack::default())
            }
            Command::React {
                portal,
                target,
                key,
            } => self.react(&portal, &target, &key).await,
            Command::Unreact { portal, target, .. } => self.react(&portal, &target, "").await,
            Command::Read { portal, event } => {
                let chat = self.chat_of(&portal)?;
                let target = match event {
                    Some(event) => event,
                    None => self
                        .last_seen
                        .get(&portal.id)
                        .cloned()
                        .ok_or(Error::Unsupported("reading a chat with no messages"))?,
                };
                let parsed = crate::ids::parse_message_id(&target)
                    .ok_or(Error::Unsupported("marking that message as read"))?;
                self.send_read(&chat, &parsed).await?;
                Ok(Ack::default())
            }
            Command::Typing { portal, typing } => {
                let chat = self.chat_of(&portal)?;
                let state = if typing { "composing" } else { "paused" };
                let node = Node::new("chatstate")
                    .attr("to", chat.to_string())
                    .children(vec![Node::new(state)]);
                self.send(&node).await?;
                Ok(Ack::default())
            }
        }
    }

    fn ack_for_sent(&self, chat: &Jid, id: &str) -> Ack {
        Ack {
            remote_event_id: Some(ids::message_id(chat, &self.own, id)),
        }
    }

    async fn react(&mut self, portal: &PortalRef, target: &str, key: &str) -> Result<Ack> {
        let chat = self.chat_of(portal)?;
        let target = crate::ids::parse_message_id(target)
            .ok_or(Error::Unsupported("reacting to that message"))?;
        let message = crate::convert::reaction_message(&chat, &target, &self.own, key);
        self.send_new(&chat, message, "").await?;
        Ok(Ack::default())
    }

    /// Parses the chat of a portal id.
    pub(crate) fn chat_of(&self, portal: &PortalRef) -> Result<Jid> {
        portal
            .id
            .parse::<Jid>()
            .map_err(|_| Error::Protocol("the chat id is malformed".to_owned()))
    }

    /// Sends `message` to `chat` under a new id and returns the id.
    pub(crate) async fn send_new(
        &mut self,
        chat: &Jid,
        message: proto::e2e::Message,
        edit: &str,
    ) -> Result<String> {
        let id = generate_message_id(&mut self.rng);
        self.send_message(chat, &id, &message, edit).await?;
        Ok(id)
    }
}

#[async_trait]
impl<W: Wire + 'static> Session for Client<W> {
    async fn next(&mut self) -> Option<Event> {
        self.next_event().await
    }

    async fn command(&mut self, cmd: Command) -> Result<Ack> {
        self.dispatch(cmd).await
    }

    async fn backfill(
        &mut self,
        portal: &PortalRef,
        cursor: Option<String>,
        limit: usize,
    ) -> Result<BackfillPage> {
        self.backfill_page(portal, cursor, limit).await
    }

    async fn logout_remote(&mut self) -> Result<()> {
        let id = self.next_id();
        let request = Node::new("iq")
            .attr("to", SERVER_JID)
            .attr("type", "set")
            .attr("xmlns", "md")
            .attr("id", id)
            .children(vec![
                Node::new("remove-companion-device")
                    .attr("jid", self.own.to_string())
                    .attr("reason", "user_initiated"),
            ]);
        self.request(request).await?;
        Ok(())
    }

    fn user(&self) -> Option<RemoteUser> {
        Some(self.profile())
    }

    fn content_hint(&self) -> Option<Content> {
        None
    }
}

/// The capabilities of the WhatsApp driver: the same set as the Go build.
pub fn capabilities() -> Capabilities {
    Capabilities::ALL
}

/// A message id in WhatsApp's format: `3EB0` and eight random bytes in upper-case hex.
pub fn generate_message_id(rng: &mut impl rand::RngCore) -> String {
    let mut bytes = [0u8; 8];
    rng.fill_bytes(&mut bytes);
    format!("3EB0{}", hex::encode_upper(bytes))
}

/// A keep-alive interval between the bounds, varied by a counter so pings do not synchronise.
fn pick_interval(seed: u64) -> Duration {
    let span = u64::try_from((KEEPALIVE_MAX - KEEPALIVE_MIN).as_millis()).unwrap_or(10_000);
    let offset = seed.wrapping_mul(2_654_435_761) % span.max(1);
    KEEPALIVE_MIN + Duration::from_millis(offset)
}

/// The error of an `iq` of type `error`.
pub(crate) fn iq_error(node: &Node) -> Error {
    let code = node
        .child("error")
        .and_then(|error| error.get_attr("code"))
        .unwrap_or_default();
    match code.as_str() {
        "401" | "403" => Error::Auth("the server refused the request".to_owned()),
        "429" => Error::RateLimited {
            retry_after_ms: 60_000,
        },
        _ => Error::Protocol(format!("the server refused a request ({code})")),
    }
}

/// `<key>`-style prekey node: a 3-byte id and a 32-byte public key.
pub(crate) fn pre_key_node(record: &chirper_signal_protocol::prekey::PreKeyRecord) -> Node {
    let id = record.id().to_be_bytes();
    Node::new("key").children(vec![
        Node::new("id").bytes(id[1..].to_vec()),
        Node::new("value").bytes(record.key_pair().public_key.serialize()[1..].to_vec()),
    ])
}

/// The signed prekey node: a 3-byte id, the 32-byte public key and the 64-byte signature.
pub(crate) fn signed_pre_key_node(
    record: &chirper_signal_protocol::prekey::SignedPreKeyRecord,
) -> Node {
    let id = record.id().to_be_bytes();
    Node::new("skey").children(vec![
        Node::new("id").bytes(id[1..].to_vec()),
        Node::new("value").bytes(record.key_pair().public_key.serialize()[1..].to_vec()),
        Node::new("signature").bytes(record.signature().to_vec()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keepalive_intervals_stay_between_the_bounds() {
        for seed in 0..200u64 {
            let interval = pick_interval(seed);
            assert!(
                interval >= KEEPALIVE_MIN && interval < KEEPALIVE_MAX,
                "{interval:?}"
            );
        }
    }

    #[test]
    fn one_reaction_per_reactor_and_an_empty_emoji_removes_it() {
        let mut reactions = Reactions::default();
        reactions.set("m1", "alice", "👍");
        let set = reactions.set("m1", "alice", "❤️");
        assert_eq!(set, vec![("alice".to_owned(), "❤️".to_owned())]);
        let set = reactions.set("m1", "alice", "");
        assert!(set.is_empty());
    }
}
