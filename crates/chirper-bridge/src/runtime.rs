//! The bridge runtime: one supervisor per login, one queue per login, one place where remote ids
//! become local ids.
//!
//! What this replaces from the Go stack: bridgev2's `Bridge`/`MatrixConnector`/`Intent` plumbing,
//! `localmx`'s Matrix emulation, and the per-bridge `bridge-*.db` bookkeeping. The runtime owns
//! reconnection, backoff, rate limiting, capability gating and event routing; a driver only speaks
//! its network's protocol.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use log::{debug, info, warn};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::ids::{self, ChatId, EventId, SenderId};
use crate::model::{
    Ack, BackfillPage, ChatMeta, DeliveryStatus, Event, PortalRef, RemoteEventRef, RemoteMessage,
    RemoteUser, SessionState,
};
use crate::network::{Command, Error, LoginCtx, Network, Result, Session};

/// A driver event after the runtime assigned local ids.
#[derive(Debug)]
pub enum Routed {
    Chat {
        chat: ChatId,
        portal: PortalRef,
        meta: ChatMeta,
    },
    ChatRemoved {
        chat: ChatId,
        portal: PortalRef,
    },
    Message {
        chat: ChatId,
        portal: PortalRef,
        event: EventId,
        sender: SenderId,
        message: RemoteMessage,
        /// When set, this message is an edit of that local event.
        edit_target: Option<EventId>,
        /// The local event this message replies to.
        reply_to: Option<EventId>,
    },
    Redact {
        chat: ChatId,
        portal: PortalRef,
        target: EventId,
    },
    Reactions {
        chat: ChatId,
        target: EventId,
        reactions: Vec<crate::model::Reaction>,
    },
    Typing {
        chat: ChatId,
        senders: Vec<SenderId>,
        /// Display names, so the chat list does not need a member lookup per keystroke.
        names: Vec<String>,
    },
    Member {
        chat: ChatId,
        sender: SenderId,
        user: RemoteUser,
        membership: String,
    },
    ReadReceipt {
        chat: ChatId,
        sender: SenderId,
        target: EventId,
    },
    Delivery {
        chat: ChatId,
        txn_id: String,
        status: DeliveryStatus,
        error: String,
    },
    State {
        state: SessionState,
        error: String,
        message: String,
        /// The logged-in user, when the network reported it.
        profile: Option<(SenderId, RemoteUser)>,
    },
    Error(String),
}

/// A routed event, tagged with the login it came from.
#[derive(Debug)]
pub struct RoutedEvent {
    pub network: String,
    pub login_id: String,
    pub routed: Routed,
}

/// An operation the supervisor runs against the live session.
enum Op {
    Command {
        cmd: Command,
        reply: oneshot::Sender<Result<Ack>>,
    },
    Backfill {
        portal: PortalRef,
        cursor: Option<String>,
        limit: usize,
        reply: oneshot::Sender<Result<BackfillPage>>,
    },
    Logout {
        reply: oneshot::Sender<Result<()>>,
    },
    Stop,
}

struct LoginHandle {
    ops: mpsc::UnboundedSender<Op>,
    /// Set when the supervisor stopped on purpose, so late commands say "not connected".
    stopped: Arc<std::sync::atomic::AtomicBool>,
    task: Mutex<Option<JoinHandle<()>>>,
}

/// The unified bridge runtime.
pub struct Runtime {
    networks: Vec<Arc<dyn Network>>,
    by_id: HashMap<&'static str, Arc<dyn Network>>,
    logins: Mutex<HashMap<(String, String), LoginHandle>>,
    sink: mpsc::UnboundedSender<RoutedEvent>,
    limiters: Mutex<HashMap<String, Instant>>,
}

/// Minimum gap between two commands of one network. Drivers still handle a real 429; this only
/// keeps a burst of user actions from turning into one.
const COMMAND_GAP: Duration = Duration::from_millis(40);
/// First reconnect delay; doubles up to [`MAX_BACKOFF`].
const MIN_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(120);

impl Runtime {
    pub fn new(
        networks: Vec<Arc<dyn Network>>,
        sink: mpsc::UnboundedSender<RoutedEvent>,
    ) -> Runtime {
        let by_id = networks
            .iter()
            .map(|n| (n.id(), Arc::clone(n)))
            .collect::<HashMap<_, _>>();
        Runtime {
            networks,
            by_id,
            logins: Mutex::new(HashMap::new()),
            sink,
            limiters: Mutex::new(HashMap::new()),
        }
    }

    pub fn networks(&self) -> &[Arc<dyn Network>] {
        &self.networks
    }

    pub fn network(&self, id: &str) -> Option<Arc<dyn Network>> {
        self.by_id.get(id).cloned()
    }

    /// Start the supervisor of a login. Idempotent: a running login is left alone, and a stopped
    /// one is restarted.
    pub fn start_login(&self, network: &str, login_id: &str, ctx: LoginCtx) {
        let Some(driver) = self.network(network) else {
            warn!("start_login for unknown network {network}");
            return;
        };
        let key = (network.to_owned(), login_id.to_owned());
        let mut logins = self.logins.lock().expect("bridge login map");
        if let Some(handle) = logins.get(&key) {
            if !handle.stopped.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
        }
        let (ops, ops_rx) = mpsc::unbounded_channel();
        let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let task = tokio::spawn(supervise(
            driver,
            ctx,
            self.sink.clone(),
            ops_rx,
            Arc::clone(&stopped),
        ));
        logins.insert(
            key,
            LoginHandle {
                ops,
                stopped,
                task: Mutex::new(Some(task)),
            },
        );
    }

    /// Stop a login's supervisor. It does not log out anywhere; that is [`Runtime::logout_remote`].
    pub fn stop_login(&self, network: &str, login_id: &str) {
        let handle = self
            .logins
            .lock()
            .expect("bridge login map")
            .remove(&(network.to_owned(), login_id.to_owned()));
        if let Some(handle) = handle {
            handle
                .stopped
                .store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = handle.ops.send(Op::Stop);
            if let Some(task) = handle.task.lock().expect("task handle").take() {
                task.abort();
            }
        }
    }

    /// Stop every supervisor. The engine calls this on shutdown.
    pub fn stop_all(&self) {
        let logins = std::mem::take(&mut *self.logins.lock().expect("bridge login map"));
        for (_, handle) in logins {
            handle
                .stopped
                .store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = handle.ops.send(Op::Stop);
            if let Some(task) = handle.task.lock().expect("task handle").take() {
                task.abort();
            }
        }
    }

    /// Whether the login has a running supervisor.
    pub fn is_running(&self, network: &str, login_id: &str) -> bool {
        self.logins
            .lock()
            .expect("bridge login map")
            .get(&(network.to_owned(), login_id.to_owned()))
            .is_some_and(|h| !h.stopped.load(std::sync::atomic::Ordering::SeqCst))
    }

    fn ops(&self, network: &str, login_id: &str) -> Result<mpsc::UnboundedSender<Op>> {
        self.logins
            .lock()
            .expect("bridge login map")
            .get(&(network.to_owned(), login_id.to_owned()))
            .filter(|h| !h.stopped.load(std::sync::atomic::Ordering::SeqCst))
            .map(|h| h.ops.clone())
            .ok_or(Error::NotConnected)
    }

    /// Wait until the network's command gap has passed, so a burst does not become a burst of
    /// requests.
    async fn pace(&self, network: &str) {
        let wait = {
            let mut limiters = self.limiters.lock().expect("rate limiter");
            let now = Instant::now();
            let next = limiters.entry(network.to_owned()).or_insert(now);
            let start = (*next).max(now);
            *next = start + COMMAND_GAP;
            start.saturating_duration_since(now)
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }

    /// Whether the network's capabilities allow this command, before it reaches the driver.
    fn gate(driver: &dyn Network, cmd: &Command) -> Result<()> {
        let caps = driver.capabilities();
        let missing = match cmd {
            Command::SendText { reply_to, .. } | Command::SendMedia { reply_to, .. } => {
                reply_to.as_ref().filter(|_| !caps.replies).map(|_| "replies")
            }
            Command::Edit { .. } => (!caps.edits).then_some("edits"),
            Command::Redact { .. } => (!caps.redactions).then_some("redactions"),
            Command::React { .. } | Command::Unreact { .. } => {
                (!caps.reactions).then_some("reactions")
            }
            Command::Read { .. } => (!caps.read_receipts).then_some("read receipts"),
            Command::Typing { .. } => (!caps.typing).then_some("typing"),
        };
        match missing {
            Some(name) => Err(Error::Unsupported(name)),
            None => Ok(()),
        }
    }

    pub async fn command(&self, network: &str, login_id: &str, cmd: Command) -> Result<Ack> {
        let driver = self.network(network).ok_or(Error::NotConnected)?;
        Self::gate(driver.as_ref(), &cmd)?;
        let ops = self.ops(network, login_id)?;
        self.pace(network).await;
        let (reply, reply_rx) = oneshot::channel();
        ops.send(Op::Command { cmd, reply })
            .map_err(|_| Error::NotConnected)?;
        reply_rx.await.map_err(|_| Error::NotConnected)?
    }

    pub async fn backfill(
        &self,
        network: &str,
        login_id: &str,
        portal: PortalRef,
        cursor: Option<String>,
        limit: usize,
    ) -> Result<BackfillPage> {
        let ops = self.ops(network, login_id)?;
        let (reply, reply_rx) = oneshot::channel();
        ops.send(Op::Backfill {
            portal,
            cursor,
            limit,
            reply,
        })
        .map_err(|_| Error::NotConnected)?;
        reply_rx.await.map_err(|_| Error::NotConnected)?
    }

    pub async fn logout_remote(&self, network: &str, login_id: &str) -> Result<()> {
        let ops = self.ops(network, login_id)?;
        let (reply, reply_rx) = oneshot::channel();
        ops.send(Op::Logout { reply })
            .map_err(|_| Error::NotConnected)?;
        reply_rx.await.map_err(|_| Error::NotConnected)?
    }
}

/// Run one login: connect, serve, reconnect, until stopped or until the credentials are gone.
async fn supervise(
    driver: Arc<dyn Network>,
    ctx: LoginCtx,
    sink: mpsc::UnboundedSender<RoutedEvent>,
    mut ops: mpsc::UnboundedReceiver<Op>,
    stopped: Arc<std::sync::atomic::AtomicBool>,
) {
    let network = driver.id().to_owned();
    let login_id = ctx.login_id.clone();
    let mut backoff = MIN_BACKOFF;

    let emit = |routed: Routed| -> bool {
        sink.send(RoutedEvent {
            network: network.clone(),
            login_id: login_id.clone(),
            routed,
        })
        .is_ok()
    };

    loop {
        if stopped.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        emit(Routed::State {
            state: SessionState::Connecting,
            error: String::new(),
            message: String::new(),
            profile: None,
        });
        match driver.connect(ctx.clone()).await {
            Ok(mut session) => {
                info!("{network}/{login_id}: connected");
                backoff = MIN_BACKOFF;
                emit(Routed::State {
                    state: SessionState::Running,
                    error: String::new(),
                    message: String::new(),
                    profile: session.user().map(|user| (ids::sender_id(&network, &user.id), user)),
                });

                let reason = serve(&network, &mut ops, &mut *session, &emit).await;
                if reason.is_some_and(SessionState::is_terminal) {
                    stopped.store(true, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
                if stopped.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                debug!("{network}/{login_id}: connection ended, reconnecting");
            }
            Err(err) => {
                warn!("{network}/{login_id}: connect failed: {err}");
                let state = err.session_state();
                emit(Routed::State {
                    state,
                    error: err.to_string(),
                    message: String::new(),
                    profile: None,
                });
                if !err.is_retryable() {
                    stopped.store(true, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            }
        }

        // Back off, but stay responsive to Stop and to commands (which fail as not connected).
        let delay = jitter(backoff);
        let sleep = tokio::time::sleep(delay);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                () = &mut sleep => break,
                op = ops.recv() => match op {
                    Some(Op::Stop) | None => return,
                    Some(Op::Command { reply, .. }) => { let _ = reply.send(Err(Error::NotConnected)); }
                    Some(Op::Backfill { reply, .. }) => { let _ = reply.send(Err(Error::NotConnected)); }
                    Some(Op::Logout { reply }) => { let _ = reply.send(Err(Error::NotConnected)); }
                }
            }
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Serve one live session until it ends. Returns the terminal state the driver reported, if any.
async fn serve(
    network: &str,
    ops: &mut mpsc::UnboundedReceiver<Op>,
    session: &mut dyn Session,
    emit: &impl Fn(Routed) -> bool,
) -> Option<SessionState> {
    let mut terminal = None;
    loop {
        tokio::select! {
            event = session.next() => match event {
                Some(event) => {
                    if let Event::State { state, .. } = &event {
                        if state.is_terminal() {
                            terminal = Some(*state);
                        }
                    }
                    route(network, event, emit);
                    if terminal.is_some() {
                        return terminal;
                    }
                }
                None => return terminal,
            },
            op = ops.recv() => match op {
                Some(Op::Command { cmd, reply }) => {
                    let result = session.command(cmd).await;
                    let _ = reply.send(result);
                }
                Some(Op::Backfill { portal, cursor, limit, reply }) => {
                    let result = session.backfill(&portal, cursor, limit).await;
                    let _ = reply.send(result);
                }
                Some(Op::Logout { reply }) => {
                    let result = session.logout_remote().await;
                    let _ = reply.send(result);
                }
                Some(Op::Stop) | None => return terminal,
            },
        }
    }
}

/// Assign local ids and forward one driver event.
fn route(network: &str, event: Event, emit: &impl Fn(Routed) -> bool) {
    let chat_of = |portal: &PortalRef| ids::chat_id(network, &portal.id, &portal.receiver);
    let event_of = |chat: &ChatId, r: &RemoteEventRef| {
        if r.id.is_empty() {
            EventId::default()
        } else {
            ids::event_id_for(network, chat.as_str(), &r.id, &r.part)
        }
    };

    let routed = match event {
        Event::Chat(meta) => {
            let chat = chat_of(&meta.portal);
            Routed::Chat {
                chat,
                portal: meta.portal.clone(),
                meta,
            }
        }
        Event::ChatRemoved(portal) => {
            let chat = chat_of(&portal);
            Routed::ChatRemoved { chat, portal }
        }
        Event::Message(message) => {
            let chat = chat_of(&message.portal);
            let event = ids::event_id_for(
                network,
                chat.as_str(),
                &message.id,
                &message.part,
            );
            let sender = ids::sender_id(network, &message.sender.id);
            let edit_target = message.edit_of.as_ref().map(|target| {
                // An edit names the message it replaces; parts are not involved.
                ids::event_id_for(network, chat.as_str(), target, "")
            });
            let reply_to = message
                .reply_to
                .as_ref()
                .map(|target| ids::event_id_for(network, chat.as_str(), target, ""));
            Routed::Message {
                chat,
                portal: message.portal.clone(),
                event,
                sender,
                message,
                edit_target,
                reply_to,
            }
        }
        Event::Redact { portal, event } => {
            let chat = chat_of(&portal);
            Routed::Redact {
                target: event_of(&chat, &event),
                chat,
                portal,
            }
        }
        Event::Reactions {
            portal,
            target,
            reactions,
        } => {
            let chat = chat_of(&portal);
            let target = event_of(&chat, &target);
            Routed::Reactions {
                chat,
                target,
                reactions,
            }
        }
        Event::Typing { portal, senders } => {
            let chat = chat_of(&portal);
            let names = senders
                .iter()
                .map(|user| {
                    if user.display_name.is_empty() {
                        user.id.clone()
                    } else {
                        user.display_name.clone()
                    }
                })
                .collect();
            let senders = senders
                .iter()
                .map(|user| ids::sender_id(network, &user.id))
                .collect();
            Routed::Typing {
                chat,
                senders,
                names,
            }
        }
        Event::Member { portal, member } => {
            let chat = chat_of(&portal);
            Routed::Member {
                sender: ids::sender_id(network, &member.user.id),
                chat,
                membership: member.membership.clone(),
                user: member.user,
            }
        }
        Event::ReadReceipt { portal, user, event } => {
            let chat = chat_of(&portal);
            Routed::ReadReceipt {
                target: event_of(&chat, &event),
                sender: ids::sender_id(network, &user.id),
                chat,
            }
        }
        Event::Delivery {
            portal,
            txn_id,
            status,
            error,
        } => Routed::Delivery {
            chat: chat_of(&portal),
            txn_id,
            status,
            error,
        },
        Event::State {
            state,
            error,
            message,
            profile,
        } => Routed::State {
            state,
            error,
            message,
            profile: profile.map(|user| (ids::sender_id(network, &user.id), user)),
        },
        Event::Error(message) => Routed::Error(message),
    };
    emit(routed);
}

/// Add up to 20% jitter, so that many logins do not reconnect in lockstep.
fn jitter(delay: Duration) -> Duration {
    let mut bytes = [0u8; 2];
    let _ = getrandom::fill(&mut bytes);
    let spread = (delay.as_millis() / 5).max(1) as u64;
    delay + Duration::from_millis((u16::from_le_bytes(bytes) as u64) % spread)
}
