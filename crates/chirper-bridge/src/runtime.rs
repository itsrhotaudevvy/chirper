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
use crate::redact::redact;

/// A driver event after the runtime assigned local ids.
#[derive(Debug)]
pub enum Routed {
    Chat {
        chat: ChatId,
        portal: PortalRef,
        meta: Box<ChatMeta>,
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
        message: Box<RemoteMessage>,
        /// When set, this message is an edit of that local event.
        edit_target: Option<EventId>,
        /// The local event this message replies to.
        reply_to: Option<EventId>,
    },
    Redact {
        chat: ChatId,
        portal: PortalRef,
        target: EventId,
        /// The network's id of the redacted event, for finding an outgoing message by it.
        remote: String,
    },
    Reactions {
        chat: ChatId,
        target: EventId,
        /// The network's id of the reacted-to message.
        remote: String,
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
        /// The network's id of the message read up to.
        remote: String,
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
    ops: mpsc::Sender<Op>,
    /// Set when the supervisor stopped on purpose, so late commands say "not connected".
    stopped: Arc<std::sync::atomic::AtomicBool>,
    task: Mutex<Option<JoinHandle<()>>>,
}

/// The unified bridge runtime.
pub struct Runtime {
    networks: Vec<Arc<dyn Network>>,
    by_id: HashMap<&'static str, Arc<dyn Network>>,
    logins: Mutex<HashMap<(String, String), LoginHandle>>,
    sink: mpsc::Sender<RoutedEvent>,
    limiters: Mutex<HashMap<String, Instant>>,
}

/// Minimum gap between two commands of one network. Drivers still handle a real 429; this only
/// keeps a burst of user actions from turning into one.
const COMMAND_GAP: Duration = Duration::from_millis(40);
/// First reconnect delay; doubles up to [`MAX_BACKOFF`].
const MIN_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(120);
/// Longest wait a network may ask for with `RateLimited { retry_after_ms }`.
const MAX_RATE_LIMIT_WAIT: Duration = Duration::from_secs(15 * 60);
/// Commands and backfill requests a login may have queued before callers wait.
pub const OPS_QUEUE: usize = 64;
/// Driver events a login may have queued for the engine. A full queue slows the driver down
/// (its socket stops being read) instead of letting memory grow with the flood.
pub const EVENT_QUEUE: usize = 256;

impl Runtime {
    pub fn new(networks: Vec<Arc<dyn Network>>, sink: mpsc::Sender<RoutedEvent>) -> Runtime {
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
        if let Some(handle) = logins.get(&key)
            && !handle.stopped.load(std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let (ops, ops_rx) = mpsc::channel(OPS_QUEUE);
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
            // A full queue cannot hold the stop; the abort below ends the supervisor anyway.
            let _ = handle.ops.try_send(Op::Stop);
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
            // A full queue cannot hold the stop; the abort below ends the supervisor anyway.
            let _ = handle.ops.try_send(Op::Stop);
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

    fn ops(&self, network: &str, login_id: &str) -> Result<mpsc::Sender<Op>> {
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
            Command::SendText { reply_to, .. } | Command::SendMedia { reply_to, .. } => reply_to
                .as_ref()
                .filter(|_| !caps.replies)
                .map(|_| "replies"),
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
            .await
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
        .await
        .map_err(|_| Error::NotConnected)?;
        reply_rx.await.map_err(|_| Error::NotConnected)?
    }

    pub async fn logout_remote(&self, network: &str, login_id: &str) -> Result<()> {
        let ops = self.ops(network, login_id)?;
        let (reply, reply_rx) = oneshot::channel();
        ops.send(Op::Logout { reply })
            .await
            .map_err(|_| Error::NotConnected)?;
        reply_rx.await.map_err(|_| Error::NotConnected)?
    }
}

/// The one way a login's events reach the engine. Sending waits while the engine is behind.
struct Emitter {
    sink: mpsc::Sender<RoutedEvent>,
    network: String,
    login_id: String,
}

impl Emitter {
    /// Hand one routed event over. `false` once the engine has gone away.
    async fn send(&self, routed: Routed) -> bool {
        self.sink
            .send(RoutedEvent {
                network: self.network.clone(),
                login_id: self.login_id.clone(),
                routed,
            })
            .await
            .is_ok()
    }
}

/// Run one login: connect, serve, reconnect, until stopped or until the credentials are gone.
async fn supervise(
    driver: Arc<dyn Network>,
    ctx: LoginCtx,
    sink: mpsc::Sender<RoutedEvent>,
    mut ops: mpsc::Receiver<Op>,
    stopped: Arc<std::sync::atomic::AtomicBool>,
) {
    let network = driver.id().to_owned();
    let login_id = ctx.login_id.clone();
    let emitter = Emitter {
        sink,
        network: network.clone(),
        login_id: login_id.clone(),
    };
    let mut backoff = MIN_BACKOFF;

    loop {
        if stopped.load(std::sync::atomic::Ordering::SeqCst) || emitter.sink.is_closed() {
            return;
        }
        // The wait a `RateLimited` failure asked for. It only applies to the reconnect after it.
        let mut retry_after: Option<Duration> = None;
        let connecting = Routed::State {
            state: SessionState::Connecting,
            error: String::new(),
            message: String::new(),
            profile: None,
        };
        if !emitter.send(connecting).await {
            return;
        }
        match driver.connect(ctx.clone()).await {
            Ok(mut session) => {
                info!("{network}/{login_id}: connected");
                backoff = MIN_BACKOFF;
                let running = Routed::State {
                    state: SessionState::Running,
                    error: String::new(),
                    message: String::new(),
                    profile: session
                        .user()
                        .map(|user| (ids::sender_id(&network, &user.id), user)),
                };
                if !emitter.send(running).await {
                    return;
                }

                let reason = serve(&network, &mut ops, &mut *session, &emitter).await;
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
                let text = redact(&err.to_string());
                warn!("{network}/{login_id}: connect failed: {text}");
                let failed = Routed::State {
                    state: err.session_state(),
                    error: text,
                    message: String::new(),
                    profile: None,
                };
                if !emitter.send(failed).await {
                    return;
                }
                if !err.is_retryable() {
                    stopped.store(true, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
                retry_after = match err {
                    Error::RateLimited { retry_after_ms } => {
                        Some(Duration::from_millis(retry_after_ms))
                    }
                    _ => None,
                };
            }
        }

        // Back off, but stay responsive to Stop and to commands (which fail as not connected).
        let delay = next_delay(backoff, retry_after);
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
    ops: &mut mpsc::Receiver<Op>,
    session: &mut dyn Session,
    emitter: &Emitter,
) -> Option<SessionState> {
    let mut terminal = None;
    loop {
        tokio::select! {
            event = session.next() => match event {
                Some(event) => {
                    if let Event::State { state, .. } = &event
                        && state.is_terminal()
                    {
                        terminal = Some(*state);
                    }
                    if !emitter.send(route(network, event)).await {
                        return terminal;
                    }
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

/// Assign local ids to one driver event. Remote text is redacted here, once, for every consumer.
fn route(network: &str, event: Event) -> Routed {
    let chat_of = |portal: &PortalRef| ids::chat_id(network, &portal.id, &portal.receiver);
    let event_of = |chat: &ChatId, r: &RemoteEventRef| {
        if r.id.is_empty() {
            EventId::default()
        } else {
            ids::event_id_for(network, chat.as_str(), &r.id, &r.part)
        }
    };

    match event {
        Event::Chat(meta) => {
            let chat = chat_of(&meta.portal);
            Routed::Chat {
                chat,
                portal: meta.portal.clone(),
                meta: Box::new(meta),
            }
        }
        Event::ChatRemoved(portal) => {
            let chat = chat_of(&portal);
            Routed::ChatRemoved { chat, portal }
        }
        Event::Message(message) => {
            let chat = chat_of(&message.portal);
            let event = ids::event_id_for(network, chat.as_str(), &message.id, &message.part);
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
                message: Box::new(message),
                edit_target,
                reply_to,
            }
        }
        Event::Redact { portal, event } => {
            let chat = chat_of(&portal);
            Routed::Redact {
                target: event_of(&chat, &event),
                remote: event.id.clone(),
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
            let remote = target.id.clone();
            let target = event_of(&chat, &target);
            Routed::Reactions {
                chat,
                target,
                remote,
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
        Event::ReadReceipt {
            portal,
            user,
            event,
        } => {
            let chat = chat_of(&portal);
            Routed::ReadReceipt {
                target: event_of(&chat, &event),
                remote: event.id.clone(),
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
            error: redact(&error),
        },
        Event::State {
            state,
            error,
            message,
            profile,
        } => Routed::State {
            state,
            error: redact(&error),
            message: redact(&message),
            profile: profile.map(|user| (ids::sender_id(network, &user.id), user)),
        },
        Event::Error(message) => Routed::Error(redact(&message)),
    }
}

/// The wait before the next connect: the backoff with up to 20% jitter taken off it (so the
/// [`MAX_BACKOFF`] cap holds and logins do not reconnect in lockstep), and never less than what
/// a rate limit asked for, itself capped at [`MAX_RATE_LIMIT_WAIT`].
fn next_delay(backoff: Duration, retry_after: Option<Duration>) -> Duration {
    let delay = jitter(backoff);
    match retry_after {
        Some(wait) => delay.max(wait.min(MAX_RATE_LIMIT_WAIT)),
        None => delay,
    }
}

/// Subtract a random share of up to 20% of `delay`.
fn jitter(delay: Duration) -> Duration {
    let mut bytes = [0u8; 8];
    let _ = getrandom::fill(&mut bytes);
    let spread_ms = u64::try_from(delay.as_millis() / 5).unwrap_or(u64::MAX);
    if spread_ms == 0 {
        return delay;
    }
    let cut = u64::from_le_bytes(bytes) % spread_ms.saturating_add(1);
    delay.saturating_sub(Duration::from_millis(cut))
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex as StdMutex;

    use async_trait::async_trait;
    use tokio::time::Instant as ClockInstant;

    use super::*;
    use crate::model::{BackfillPage, Capabilities, LoginFlow, PortalRef};
    use crate::network::{LoginCtx, LoginProcess, Secrets};
    use zeroize::Zeroizing;

    /// What one scripted connect does.
    #[derive(Clone, Copy)]
    enum Plan {
        /// A transport failure, with a secret-looking word in its text.
        Down,
        /// The credentials are gone.
        Auth,
        RateLimited(u64),
        /// Connects, then the session ends at once.
        Up,
    }

    /// A driver whose connects follow a script. The last step repeats.
    struct Scripted {
        plans: StdMutex<VecDeque<Plan>>,
        attempts: Arc<StdMutex<Vec<ClockInstant>>>,
    }

    struct Ended;

    #[async_trait]
    impl Session for Ended {
        async fn next(&mut self) -> Option<Event> {
            None
        }
        async fn command(&mut self, _cmd: Command) -> Result<Ack> {
            Err(Error::NotConnected)
        }
        async fn backfill(
            &mut self,
            _portal: &PortalRef,
            _cursor: Option<String>,
            _limit: usize,
        ) -> Result<BackfillPage> {
            Ok(BackfillPage::default())
        }
        async fn logout_remote(&mut self) -> Result<()> {
            Ok(())
        }
    }

    #[async_trait]
    impl Network for Scripted {
        fn id(&self) -> &'static str {
            "scripted"
        }
        fn name(&self) -> &'static str {
            "Scripted"
        }
        fn color(&self) -> &'static str {
            "#000000"
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        fn login_flows(&self) -> Vec<LoginFlow> {
            Vec::new()
        }
        async fn start_login(&self, _flow: &str, _ctx: LoginCtx) -> Result<Box<dyn LoginProcess>> {
            Err(Error::Unsupported("login"))
        }
        async fn connect(&self, _ctx: LoginCtx) -> Result<Box<dyn Session>> {
            self.attempts.lock().unwrap().push(ClockInstant::now());
            let plan = {
                let mut plans = self.plans.lock().unwrap();
                if plans.len() > 1 {
                    plans.pop_front()
                } else {
                    plans.front().copied()
                }
            }
            .unwrap_or(Plan::Down);
            match plan {
                Plan::Down => Err(Error::Network(
                    "gateway said token=supersecret123 is wrong".to_owned(),
                )),
                Plan::Auth => Err(Error::Auth("revoked".to_owned())),
                Plan::RateLimited(retry_after_ms) => Err(Error::RateLimited { retry_after_ms }),
                Plan::Up => Ok(Box::new(Ended)),
            }
        }
    }

    struct NoSecrets;

    impl Secrets for NoSecrets {
        fn get(&self, _key: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
            Ok(None)
        }
        fn set(&self, _key: &str, _value: &[u8]) -> Result<()> {
            Ok(())
        }
        fn delete(&self, _key: &str) -> Result<()> {
            Ok(())
        }
    }

    fn ctx() -> LoginCtx {
        LoginCtx {
            network: "scripted".to_owned(),
            login_id: "l1".to_owned(),
            data_dir: std::path::PathBuf::from("."),
            secrets: Arc::new(NoSecrets),
            http: crate::http_client(),
        }
    }

    /// A runtime with one scripted login already started.
    fn start(
        plans: Vec<Plan>,
    ) -> (
        Runtime,
        mpsc::Receiver<RoutedEvent>,
        Arc<StdMutex<Vec<ClockInstant>>>,
    ) {
        let attempts = Arc::new(StdMutex::new(Vec::new()));
        let driver: Arc<dyn Network> = Arc::new(Scripted {
            plans: StdMutex::new(plans.into()),
            attempts: Arc::clone(&attempts),
        });
        let (tx, rx) = mpsc::channel(EVENT_QUEUE);
        let runtime = Runtime::new(vec![driver], tx);
        runtime.start_login("scripted", "l1", ctx());
        (runtime, rx, attempts)
    }

    /// Wait (on the paused clock) until the driver has been asked to connect `n` times.
    async fn wait_for_attempts(attempts: &StdMutex<Vec<ClockInstant>>, n: usize) {
        while attempts.lock().unwrap().len() < n {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn gaps(attempts: &StdMutex<Vec<ClockInstant>>) -> Vec<Duration> {
        let times = attempts.lock().unwrap().clone();
        times.windows(2).map(|w| w[1] - w[0]).collect()
    }

    #[tokio::test(start_paused = true)]
    async fn a_rate_limit_wait_is_honoured() {
        let (runtime, _rx, attempts) = start(vec![Plan::RateLimited(90_000), Plan::Up]);
        wait_for_attempts(&attempts, 2).await;
        let gap = gaps(&attempts)[0];
        assert!(gap >= Duration::from_secs(90), "waited {gap:?}");
        assert!(gap < Duration::from_secs(91), "waited {gap:?}");
        runtime.stop_all();
    }

    #[tokio::test(start_paused = true)]
    async fn a_rate_limit_wait_is_capped_at_fifteen_minutes() {
        let (runtime, _rx, attempts) =
            start(vec![Plan::RateLimited(10 * 60 * 60 * 1000), Plan::Up]);
        wait_for_attempts(&attempts, 2).await;
        let gap = gaps(&attempts)[0];
        assert!(gap >= Duration::from_secs(15 * 60), "waited {gap:?}");
        assert!(gap < Duration::from_secs(15 * 60 + 1), "waited {gap:?}");
        runtime.stop_all();
    }

    #[tokio::test(start_paused = true)]
    async fn reconnect_backoff_doubles_with_jitter_and_stays_under_the_cap() {
        let (runtime, _rx, attempts) = start(vec![Plan::Down]);
        wait_for_attempts(&attempts, 11).await;
        for (k, gap) in gaps(&attempts).into_iter().enumerate() {
            let base = Duration::from_secs(1u64 << k.min(7)).min(MAX_BACKOFF);
            assert!(gap <= base, "attempt {k}: waited {gap:?}, base {base:?}");
            assert!(
                gap + Duration::from_millis(1) >= base.mul_f64(0.8),
                "attempt {k}: waited {gap:?}, base {base:?}"
            );
            assert!(gap <= MAX_BACKOFF, "attempt {k}: {gap:?} is over the cap");
        }
        runtime.stop_all();
    }

    #[tokio::test(start_paused = true)]
    async fn an_auth_failure_is_not_retried() {
        let (runtime, mut rx, attempts) = start(vec![Plan::Auth]);
        let mut states = Vec::new();
        while let Some(event) = rx.recv().await {
            if let Routed::State { state, .. } = event.routed {
                states.push(state);
                if state == SessionState::NeedsReauth {
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(60 * 60)).await;
        assert_eq!(attempts.lock().unwrap().len(), 1, "retried an auth failure");
        assert!(!runtime.is_running("scripted", "l1"));
        runtime.stop_all();
    }

    #[tokio::test(start_paused = true)]
    async fn driver_error_text_is_redacted_before_it_leaves_the_runtime() {
        let (runtime, mut rx, attempts) = start(vec![Plan::Down]);
        wait_for_attempts(&attempts, 1).await;
        let mut seen = Vec::new();
        while seen.is_empty() {
            let event = rx.recv().await.expect("an event");
            if let Routed::State { error, .. } = event.routed
                && !error.is_empty()
            {
                seen.push(error);
            }
        }
        assert!(!seen[0].contains("supersecret123"), "{}", seen[0]);
        assert!(seen[0].contains("gateway said"), "{}", seen[0]);
        runtime.stop_all();
    }

    #[test]
    fn driver_error_events_are_redacted_by_route() {
        let routed = route(
            "scripted",
            Event::Error("upstream rejected Authorization: Bearer zz9.yy8-xx7_ww6".to_owned()),
        );
        match routed {
            Routed::Error(message) => {
                assert!(!message.contains("zz9.yy8"), "{message}");
                assert!(message.contains("upstream rejected"), "{message}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_full_event_queue_makes_the_driver_wait() {
        let (tx, mut rx) = mpsc::channel(2);
        let emitter = Emitter {
            sink: tx,
            network: "scripted".to_owned(),
            login_id: "l1".to_owned(),
        };
        let state = || Routed::Error("x".to_owned());
        assert!(emitter.send(state()).await);
        assert!(emitter.send(state()).await);
        let third = tokio::time::timeout(Duration::from_secs(1), emitter.send(state())).await;
        assert!(
            third.is_err(),
            "the third event did not wait for the engine"
        );
        rx.recv().await.expect("an event");
        assert!(emitter.send(state()).await);
    }

    #[test]
    fn the_retry_after_wait_never_drops_below_the_backoff() {
        let short = next_delay(Duration::from_secs(8), Some(Duration::from_secs(1)));
        assert!(short >= Duration::from_secs(6) && short <= Duration::from_secs(8));
        let long = next_delay(Duration::from_secs(8), Some(Duration::from_secs(30)));
        assert_eq!(long, Duration::from_secs(30));
    }
}
