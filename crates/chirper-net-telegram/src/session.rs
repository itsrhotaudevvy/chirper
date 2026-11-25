//! The live session: connect-time sync, the update pipeline and the event loop.
//!
//! [`Session::start`] runs the sync before the runtime sees the account: the update state (from
//! `updates.getState`, or `updates.getDifference` when a position is stored), then every dialog
//! (`messages.getDialogs`, folders 0 and 1). [`Session::next`] then delivers events from the
//! server pushes. A gap in `pts`, `seq` or a channel `pts` waits half a second, then fetches the
//! difference; duplicates are dropped.
//!
//! Outgoing commands and backfill live in `commands.rs` and `backfill.rs`, as further `impl`
//! blocks on this type.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use chirper_bridge::{
    ChatMeta, Content, DeliveryStatus, Error, Event, PortalRef, Reaction, RemoteEventRef,
    RemoteMessage, RemoteUser, Result, SessionState,
};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::client::{Api, Incoming};
use crate::convert::{self, Body};
use crate::ids::{self, PeerKind};
use crate::mtproto::UpdateFrame;
use crate::peers;
use crate::state::{Check, Position, advance_pts, advance_seq, check_pts, check_seq};
use crate::store::Store;
use crate::tl::{Cursor, Deserializable, enums, functions, types};

/// How long a detected gap waits before the difference is fetched.
pub const GAP_WAIT: Duration = Duration::from_millis(500);
/// How long to wait before retrying a difference that failed for a transient reason.
pub const RETRY_WAIT: Duration = Duration::from_secs(5);
/// Dialogs per `messages.getDialogs` page.
pub const DIALOG_PAGE: i32 = 100;
/// Messages per `updates.getChannelDifference` page.
const CHANNEL_DIFFERENCE_LIMIT: i32 = 100;
/// Most message ids remembered for deletions that carry no peer.
const RECENT_CAP: usize = 10_000;

/// Message id to the portal it was seen in, for deletions that name no peer.
#[derive(Debug, Default)]
pub struct Recent {
    map: BTreeMap<String, PortalRef>,
    order: VecDeque<String>,
}

impl Recent {
    fn remember(&mut self, id: &str, portal: &PortalRef) {
        if self.map.insert(id.to_owned(), portal.clone()).is_none() {
            self.order.push_back(id.to_owned());
        }
        while self.order.len() > RECENT_CAP {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }

    fn find(&self, id: &str) -> Option<PortalRef> {
        self.map.get(id).cloned()
    }
}

/// One live login: the server connection, its update state and the events not yet delivered.
pub struct Session<A: Api> {
    pub(crate) api: A,
    pub(crate) store: Arc<Store>,
    /// The account's user id.
    pub(crate) me: i64,
    pub(crate) profile: RemoteUser,
    incoming: mpsc::UnboundedReceiver<Incoming>,
    pub(crate) pending: VecDeque<Event>,
    recent: Recent,
    gap_at: Option<Instant>,
    common_gap: bool,
    channel_gaps: BTreeSet<i64>,
    closed: bool,
    is_bot: bool,
    sync: Sync,
    /// Earliest time the next sync step may run after a failure.
    retry_at: Option<Instant>,
}

/// The user id of a peer when it is a user.
fn user_of(peer: &enums::Peer) -> Option<i64> {
    match peer {
        enums::Peer::PeerUser(p) => Some(p.user_id),
        _ => None,
    }
}

/// The value of a `pts` check that carries no count: a newer `pts` is applied, an older one is
/// a duplicate.
fn check_pts_only(local: i32, pts: i32) -> Check {
    if local == 0 || pts > local {
        Check::Apply
    } else {
        Check::Duplicate
    }
}

/// The parts of a message that are not its peer or id.
pub(crate) struct MessageParts {
    pub sender: RemoteUser,
    pub date: i32,
    pub content: Content,
    pub reply: Option<convert::ReplyTarget>,
    pub topic: i32,
    pub live: bool,
    pub from_me: bool,
    pub edit: bool,
}

/// The flow of a failed gate: a duplicate is skipped, a gap stops the container.
fn flow(check: Check) -> Flow {
    match check {
        Check::Gap => Flow::Gap,
        Check::Duplicate | Check::Apply => Flow::Skipped,
    }
}

/// Where the connect-time sync stands. It lives in the session, so a dropped `next()` resumes here.
#[derive(Clone, Debug)]
enum Sync {
    /// Catch up on the updates missed while disconnected.
    Difference,
    /// Fetch the dialogs, one page at a time.
    Dialogs(DialogCursor),
    /// The sync is complete: live updates flow.
    Done,
}

/// The paging position in `messages.getDialogs`.
#[derive(Clone, Debug)]
struct DialogCursor {
    folder: i32,
    offset_date: i32,
    offset_id: i32,
    offset_peer: enums::InputPeer,
}

impl DialogCursor {
    /// The first page of a folder.
    fn first(folder: i32) -> Self {
        Self {
            folder,
            offset_date: 0,
            offset_id: 0,
            offset_peer: enums::InputPeer::InputPeerEmpty(types::InputPeerEmpty {}),
        }
    }
}

/// The outcome of one update in the pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flow {
    Applied,
    Skipped,
    Gap,
}

/// The reactions of a message's summary; empty when the message carries none.
fn collect_reactions(summary: Option<&enums::MessageReactions>) -> Vec<Reaction> {
    match summary {
        Some(enums::MessageReactions::MessageReactions(summary)) => convert::reactions(summary),
        _ => Vec::new(),
    }
}

impl<A: Api> Session<A> {
    /// Connect the account. Authenticates (`users.getUsers` for the account itself) and returns at
    /// once. The update catch-up and the dialog sync run from [`Session::next`], so a caller is never
    /// held up by a large account; the events they produce are queued in order.
    pub async fn start(
        api: A,
        store: Arc<Store>,
        incoming: mpsc::UnboundedReceiver<Incoming>,
    ) -> Result<Session<A>> {
        let user = fetch_self(&api).await?;
        peers::remember_user(&store, &user);
        let profile = RemoteUser {
            id: ids::user_id(user.id),
            display_name: peers::user_name(&user),
            username: user.username.clone().unwrap_or_default(),
            phone: user
                .phone
                .clone()
                .map(|p| format!("+{}", p.trim_start_matches('+')))
                .unwrap_or_default(),
            ..RemoteUser::default()
        };
        let mut session = Session {
            api,
            store,
            me: user.id,
            profile,
            is_bot: user.bot,
            incoming,
            pending: VecDeque::new(),
            recent: Recent::default(),
            gap_at: None,
            common_gap: false,
            channel_gaps: BTreeSet::new(),
            closed: false,
            sync: Sync::Difference,
            retry_at: None,
        };
        session.pending.push_back(Event::State {
            state: SessionState::Connecting,
            error: String::new(),
            message: String::new(),
            profile: None,
        });
        session.store.flush()?;
        Ok(session)
    }

    /// The next event of the connection; `None` when it ended. Until the connect-time sync is done,
    /// each call advances it by one step. Dropping the future is safe at any await: the sync state
    /// and the queued events are kept in the session, and an unfinished step is simply repeated.
    pub async fn next(&mut self) -> Option<Event> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Some(event);
            }
            if self.closed {
                return None;
            }
            if !matches!(self.sync, Sync::Done) {
                let result = self.sync_step().await;
                self.report_sync(result);
                if let Err(error) = self.store.flush() {
                    log::warn!("telegram: {error}");
                }
                continue;
            }
            let gap = self.gap_at;
            tokio::select! {
                incoming = self.incoming.recv() => self.on_incoming(incoming).await,
                () = async {
                    match gap {
                        Some(at) => tokio::time::sleep_until(at).await,
                        None => std::future::pending().await,
                    }
                } => {
                    self.gap_at = None;
                    let result = self.resolve_gaps().await;
                    self.report(result);
                }
            }
            if let Err(error) = self.store.flush() {
                log::warn!("telegram: {error}");
            }
        }
    }

    /// Advance the connect-time sync by one step: the difference, or one page of dialogs.
    async fn sync_step(&mut self) -> Result<()> {
        if let Some(at) = self.retry_at.take() {
            tokio::time::sleep_until(at).await;
        }
        match self.sync.clone() {
            Sync::Difference => {
                self.sync_difference().await?;
                if self.is_bot {
                    self.finish_sync();
                } else {
                    self.begin_dialogs();
                }
                Ok(())
            }
            Sync::Dialogs(cursor) => self.dialog_page(cursor).await,
            Sync::Done => Ok(()),
        }
    }

    /// Start the dialog sync: folder 0 from the top, announced as `Backfilling`.
    fn begin_dialogs(&mut self) {
        self.sync = Sync::Dialogs(DialogCursor::first(0));
        self.pending.push_back(Event::State {
            state: SessionState::Backfilling,
            error: String::new(),
            message: String::new(),
            profile: None,
        });
    }

    /// The sync is complete: the connection is `Running` with the account's profile.
    fn finish_sync(&mut self) {
        self.sync = Sync::Done;
        self.pending.push_back(Event::State {
            state: SessionState::Running,
            error: String::new(),
            message: String::new(),
            profile: Some(self.profile.clone()),
        });
    }

    /// A failed sync step: credentials end the session, anything else is shown and retried later.
    fn report_sync(&mut self, result: Result<()>) {
        let Err(error) = result else {
            return;
        };
        if matches!(error, Error::Auth(_)) {
            self.pending.push_back(Event::State {
                state: SessionState::NeedsReauth,
                error: error.to_string(),
                message: String::new(),
                profile: None,
            });
            self.closed = true;
            return;
        }
        log::debug!("telegram: sync step failed: {error}");
        self.pending.push_back(Event::Error(error.to_string()));
        self.retry_at = Some(Instant::now() + RETRY_WAIT);
    }

    async fn on_incoming(&mut self, incoming: Option<Incoming>) {
        let home = self.api.home_dc();
        match incoming {
            None => self.closed = true,
            Some(Incoming::Closed { dc }) => {
                if dc == home {
                    self.closed = true;
                }
            }
            Some(Incoming::Frame { dc, frame }) if dc == home => {
                let result = self.handle_frame(frame).await;
                self.report(result);
            }
            Some(Incoming::Frame { .. }) => {}
        }
    }

    /// Turn a failure into events: credentials end the session, anything else is shown and retried.
    fn report(&mut self, result: Result<()>) {
        let Err(error) = result else {
            return;
        };
        if matches!(error, Error::Auth(_)) {
            self.pending.push_back(Event::State {
                state: SessionState::NeedsReauth,
                error: error.to_string(),
                message: String::new(),
                profile: None,
            });
            self.closed = true;
            return;
        }
        log::debug!("telegram: update handling failed: {error}");
        self.pending.push_back(Event::Error(error.to_string()));
        if error.is_retryable() || matches!(error, Error::Protocol(_)) {
            self.common_gap = true;
            self.gap_at = Some(Instant::now() + RETRY_WAIT);
        }
    }

    fn schedule(&mut self) {
        if self.gap_at.is_none() {
            self.gap_at = Some(Instant::now() + GAP_WAIT);
        }
    }

    async fn resolve_gaps(&mut self) -> Result<()> {
        if self.common_gap {
            self.common_gap = false;
            if let Err(error) = self.sync_difference().await {
                self.common_gap = true;
                return Err(error);
            }
        }
        let channels: Vec<i64> = self.channel_gaps.iter().copied().collect();
        for channel in channels {
            self.channel_difference(channel).await?;
            self.channel_gaps.remove(&channel);
        }
        Ok(())
    }

    async fn handle_frame(&mut self, frame: UpdateFrame) -> Result<()> {
        match frame {
            UpdateFrame::Gap => {
                self.common_gap = true;
                self.schedule();
                Ok(())
            }
            UpdateFrame::Raw { body, .. } => {
                let mut cursor = Cursor::new(&body);
                match enums::Updates::deserialize(&mut cursor) {
                    Ok(updates) => self.handle_updates(updates).await,
                    Err(_) => {
                        log::debug!("telegram: ignoring a push that is not an update");
                        Ok(())
                    }
                }
            }
        }
    }

    /// Mark the common state as having a gap, to be filled by the difference.
    fn gap_common(&mut self) {
        self.common_gap = true;
        self.schedule();
    }

    /// Apply the updates a server push carries.
    pub(crate) async fn handle_updates(&mut self, updates: enums::Updates) -> Result<()> {
        self.apply_updates(updates, true).await
    }

    /// Apply the updates an RPC answer carries (the result of our own send or edit). Their `seq`
    /// belongs to the answer, not to the pushed sequence, so only `pts` is gated.
    pub(crate) async fn handle_answer(&mut self, updates: enums::Updates) -> Result<()> {
        self.apply_updates(updates, false).await
    }

    async fn apply_updates(&mut self, updates: enums::Updates, sequenced: bool) -> Result<()> {
        match updates {
            enums::Updates::UpdatesTooLong(_) => {
                self.gap_common();
                Ok(())
            }
            enums::Updates::UpdateShortMessage(m) => {
                match self.check_common(m.pts, Some(m.pts_count)) {
                    Check::Apply => {
                        let peer = enums::Peer::PeerUser(types::PeerUser { user_id: m.user_id });
                        let from = if m.out { None } else { Some(m.user_id) };
                        let sender = self.sender_for(from, m.out);
                        let reply = m.reply_to.as_deref().and_then(convert::reply_target);
                        let text =
                            convert::text_content(&m.message, m.entities.as_deref().unwrap_or(&[]));
                        let message = self.build_message(
                            &peer,
                            m.id,
                            MessageParts {
                                sender,
                                date: m.date,
                                content: text,
                                reply,
                                topic: 0,
                                live: true,
                                from_me: m.out,
                                edit: false,
                            },
                        );
                        self.pending.push_back(Event::Message(message));
                        self.commit_common(true, m.pts);
                        Ok(())
                    }
                    _ => Ok(()),
                }
            }
            enums::Updates::UpdateShortChatMessage(m) => {
                match self.check_common(m.pts, Some(m.pts_count)) {
                    Check::Apply => {
                        let peer = enums::Peer::PeerChat(types::PeerChat { chat_id: m.chat_id });
                        let sender = self.sender_for(Some(m.from_id), m.out);
                        let reply = m.reply_to.as_deref().and_then(convert::reply_target);
                        let text =
                            convert::text_content(&m.message, m.entities.as_deref().unwrap_or(&[]));
                        let message = self.build_message(
                            &peer,
                            m.id,
                            MessageParts {
                                sender,
                                date: m.date,
                                content: text,
                                reply,
                                topic: 0,
                                live: true,
                                from_me: m.out,
                                edit: false,
                            },
                        );
                        self.pending.push_back(Event::Message(message));
                        self.commit_common(true, m.pts);
                        Ok(())
                    }
                    _ => Ok(()),
                }
            }
            enums::Updates::UpdateShortSentMessage(m) => {
                if self.check_common(m.pts, Some(m.pts_count)) == Check::Apply {
                    self.commit_common(true, m.pts);
                }
                Ok(())
            }
            enums::Updates::UpdateShort(short) => {
                self.process(*short.update, true, false).await.map(|_| ())
            }
            enums::Updates::Updates(container) => {
                self.absorb(&container.users, &container.chats);
                self.apply_container(
                    container.seq,
                    container.seq,
                    container.date,
                    container.updates,
                    sequenced,
                )
                .await
            }
            enums::Updates::UpdatesCombined(container) => {
                self.absorb(&container.users, &container.chats);
                self.apply_container(
                    container.seq_start,
                    container.seq,
                    container.date,
                    container.updates,
                    sequenced,
                )
                .await
            }
        }
    }

    async fn apply_container(
        &mut self,
        seq_start: i32,
        seq: i32,
        date: i32,
        updates: Vec<enums::Update>,
        sequenced: bool,
    ) -> Result<()> {
        if sequenced {
            let local = self.position().seq;
            match check_seq(local, seq_start, seq) {
                Check::Duplicate => return Ok(()),
                Check::Gap => {
                    self.gap_common();
                    return Ok(());
                }
                Check::Apply => {}
            }
        }
        for update in updates {
            if self.process(update, true, true).await? == Flow::Gap {
                return Ok(());
            }
        }
        if sequenced {
            let mut pos = self.position();
            advance_seq(&mut pos, seq, date);
            self.store.set_position(pos);
        }
        Ok(())
    }

    pub(crate) fn position(&self) -> Position {
        self.store.position().unwrap_or_default()
    }

    /// Check the common `pts` of an update. A gap schedules the difference.
    fn check_common(&mut self, pts: i32, count: Option<i32>) -> Check {
        let local = self.position().pts;
        let check = count.map_or_else(|| check_pts_only(local, pts), |c| check_pts(local, pts, c));
        if check == Check::Gap {
            self.gap_common();
        }
        check
    }

    /// Gate a common update when the update is live; difference updates are already in order.
    fn common_gate(&mut self, gated: bool, pts: i32, count: Option<i32>) -> Check {
        if gated {
            self.check_common(pts, count)
        } else {
            Check::Apply
        }
    }

    /// Record the common `pts` after an applied update.
    fn commit_common(&mut self, gated: bool, pts: i32) {
        if !gated {
            return;
        }
        let mut pos = self.position();
        advance_pts(&mut pos, pts);
        self.store.set_position(pos);
    }

    /// Check a channel `pts`. A channel without stored state takes the update; a gap is recorded
    /// so that the channel's difference is fetched.
    fn channel_gate(&mut self, gated: bool, channel: i64, pts: i32, count: Option<i32>) -> Check {
        if !gated {
            return Check::Apply;
        }
        let Some(local) = self.store.channel_pts(channel) else {
            self.store.set_channel_pts(channel, pts);
            return Check::Apply;
        };
        let check = count.map_or_else(|| check_pts_only(local, pts), |c| check_pts(local, pts, c));
        match check {
            Check::Apply => self.store.set_channel_pts(channel, pts.max(local)),
            Check::Gap => {
                self.channel_gaps.insert(channel);
                self.schedule();
            }
            Check::Duplicate => {}
        }
        check
    }

    /// Apply one update. `gated` checks its `pts` first.
    async fn process(&mut self, update: enums::Update, live: bool, gated: bool) -> Result<Flow> {
        match update {
            enums::Update::UpdateNewMessage(u) => {
                match self.common_gate(gated, u.pts, Some(u.pts_count)) {
                    Check::Apply => {}
                    check => return Ok(flow(check)),
                }
                self.emit_message(&u.message, live, false).await;
                self.commit_common(gated, u.pts);
                Ok(Flow::Applied)
            }
            enums::Update::UpdateEditMessage(u) => {
                match self.common_gate(gated, u.pts, Some(u.pts_count)) {
                    Check::Apply => {}
                    check => return Ok(flow(check)),
                }
                self.emit_message(&u.message, live, true).await;
                self.commit_common(gated, u.pts);
                Ok(Flow::Applied)
            }
            enums::Update::UpdateDeleteMessages(u) => {
                match self.common_gate(gated, u.pts, Some(u.pts_count)) {
                    Check::Apply => {}
                    check => return Ok(flow(check)),
                }
                for id in &u.messages {
                    self.emit_delete(ids::message_id(None, *id), None);
                }
                self.commit_common(gated, u.pts);
                Ok(Flow::Applied)
            }
            enums::Update::UpdateReadHistoryInbox(u) => {
                match self.common_gate(gated, u.pts, Some(u.pts_count)) {
                    Check::Apply => {}
                    check => return Ok(flow(check)),
                }
                if let Some(portal) = self.portal_of_peer(&u.peer, None) {
                    let event = RemoteEventRef::new(ids::message_id(None, u.max_id));
                    let user = peers::remote_user(&self.store, self.me);
                    self.pending.push_back(Event::ReadReceipt {
                        portal,
                        user,
                        event,
                    });
                }
                self.commit_common(gated, u.pts);
                Ok(Flow::Applied)
            }
            enums::Update::UpdateReadHistoryOutbox(u) => {
                match self.common_gate(gated, u.pts, Some(u.pts_count)) {
                    Check::Apply => {}
                    check => return Ok(flow(check)),
                }
                if let (Some(portal), Some(peer)) =
                    (self.portal_of_peer(&u.peer, None), user_of(&u.peer))
                {
                    let event = RemoteEventRef::new(ids::message_id(None, u.max_id));
                    let user = peers::remote_user(&self.store, peer);
                    self.pending.push_back(Event::ReadReceipt {
                        portal,
                        user,
                        event,
                    });
                }
                self.commit_common(gated, u.pts);
                Ok(Flow::Applied)
            }
            enums::Update::UpdateNewChannelMessage(u) => {
                if let Some(channel) = channel_of_message(&u.message) {
                    match self.channel_gate(gated, channel, u.pts, Some(u.pts_count)) {
                        Check::Apply => {}
                        check => return Ok(flow(check)),
                    }
                }
                self.emit_message(&u.message, live, false).await;
                Ok(Flow::Applied)
            }
            enums::Update::UpdateEditChannelMessage(u) => {
                if let Some(channel) = channel_of_message(&u.message) {
                    match self.channel_gate(gated, channel, u.pts, Some(u.pts_count)) {
                        Check::Apply => {}
                        check => return Ok(flow(check)),
                    }
                }
                self.emit_message(&u.message, live, true).await;
                Ok(Flow::Applied)
            }
            enums::Update::UpdateDeleteChannelMessages(u) => {
                match self.channel_gate(gated, u.channel_id, u.pts, Some(u.pts_count)) {
                    Check::Apply => {}
                    check => return Ok(flow(check)),
                }
                for id in &u.messages {
                    self.emit_delete(ids::message_id(Some(u.channel_id), *id), Some(u.channel_id));
                }
                Ok(Flow::Applied)
            }
            enums::Update::UpdateReadChannelInbox(u) => {
                match self.channel_gate(gated, u.channel_id, u.pts, None) {
                    Check::Apply => {}
                    check => return Ok(flow(check)),
                }
                let portal = ids::portal(PeerKind::Channel, u.channel_id, 0, "");
                let user = peers::remote_user(&self.store, self.me);
                let event = RemoteEventRef::new(ids::message_id(Some(u.channel_id), u.max_id));
                self.pending.push_back(Event::ReadReceipt {
                    portal,
                    user,
                    event,
                });
                Ok(Flow::Applied)
            }
            enums::Update::UpdateUserTyping(u) => {
                let senders = if is_cancel(&u.action) {
                    Vec::new()
                } else {
                    vec![peers::remote_user(&self.store, u.user_id)]
                };
                let portal = ids::portal(PeerKind::User, u.user_id, 0, &self.login_id());
                self.pending.push_back(Event::Typing { portal, senders });
                Ok(Flow::Applied)
            }
            enums::Update::UpdateChatUserTyping(u) => {
                let senders = if is_cancel(&u.action) {
                    Vec::new()
                } else {
                    vec![self.sender_of_peer(&u.from_id)]
                };
                let portal = ids::portal(PeerKind::Chat, u.chat_id, 0, &self.login_id());
                self.pending.push_back(Event::Typing { portal, senders });
                Ok(Flow::Applied)
            }
            enums::Update::UpdateChannelUserTyping(u) => {
                let senders = if is_cancel(&u.action) {
                    Vec::new()
                } else {
                    vec![self.sender_of_peer(&u.from_id)]
                };
                let portal = ids::portal(PeerKind::Channel, u.channel_id, 0, "");
                self.pending.push_back(Event::Typing { portal, senders });
                Ok(Flow::Applied)
            }
            enums::Update::UpdateMessageReactions(u) => {
                let channel = match u.peer.as_ref() {
                    enums::Peer::PeerChannel(p) => Some(p.channel_id),
                    _ => None,
                };
                let topic = if channel.is_some() {
                    u.top_msg_id.unwrap_or(0)
                } else {
                    0
                };
                if let Some(portal) = self.portal_of_peer(&u.peer, Some(topic)) {
                    let target = RemoteEventRef::new(ids::message_id(channel, u.msg_id));
                    let reactions = collect_reactions(Some(&u.reactions));
                    self.pending.push_back(Event::Reactions {
                        portal,
                        target,
                        reactions,
                    });
                }
                Ok(Flow::Applied)
            }
            _ => {
                log::debug!("telegram: update not shown by Chirper");
                Ok(Flow::Skipped)
            }
        }
    }

    /// Record the users and channels a server object carries: access hashes and names.
    pub(crate) fn absorb(&self, users: &[enums::User], chats: &[enums::Chat]) {
        for user in users {
            if let enums::User::User(user) = user {
                peers::remember_user(&self.store, user);
            }
        }
        for chat in chats {
            peers::remember_chat(&self.store, chat);
        }
    }

    pub(crate) fn login_id(&self) -> String {
        ids::login_id(self.me)
    }

    /// The portal of a peer. A user or chat portal carries the login id.
    pub(crate) fn portal_of_peer(
        &self,
        peer: &enums::Peer,
        topic: Option<i32>,
    ) -> Option<PortalRef> {
        let (kind, id) = convert::peer_parts(peer);
        let topic = topic.unwrap_or(0);
        Some(ids::portal(kind, id, topic, &self.login_id()))
    }

    /// The sender of a message: the author when known, the account when `out`.
    pub(crate) fn sender_for(&self, from: Option<i64>, out: bool) -> RemoteUser {
        if out {
            return self.me_user();
        }
        match from {
            Some(id) => peers::remote_user(&self.store, id),
            None => RemoteUser::default(),
        }
    }

    pub(crate) fn me_user(&self) -> RemoteUser {
        self.profile.clone()
    }

    pub(crate) fn sender_of_peer(&self, peer: &enums::Peer) -> RemoteUser {
        match peer {
            enums::Peer::PeerUser(p) => peers::remote_user(&self.store, p.user_id),
            enums::Peer::PeerChannel(p) => peers::remote_channel_user(&self.store, p.channel_id),
            enums::Peer::PeerChat(_) => RemoteUser::default(),
        }
    }

    pub(crate) fn emit_delete(&mut self, id: String, channel: Option<i64>) {
        let portal = match channel {
            Some(channel) => Some(ids::portal(PeerKind::Channel, channel, 0, "")),
            None => self.recent.find(&id),
        };
        match portal {
            Some(portal) => self.pending.push_back(Event::Redact {
                portal,
                event: RemoteEventRef::new(id),
            }),
            None => log::debug!("telegram: a deletion names a message that was never seen"),
        }
    }

    /// Emit a message from its TL object, downloading its media first.
    pub(crate) async fn emit_message(&mut self, message: &enums::Message, live: bool, edit: bool) {
        let Some(remote) = self.remote_message(message, live, edit).await else {
            return;
        };
        let reactions = match message {
            enums::Message::Message(m) => collect_reactions(m.reactions.as_deref()),
            _ => Vec::new(),
        };
        self.recent.remember(&remote.id, &remote.portal);
        if !reactions.is_empty() {
            self.pending.push_back(Event::Reactions {
                portal: remote.portal.clone(),
                target: RemoteEventRef::new(remote.id.clone()),
                reactions,
            });
        }
        self.pending.push_back(Event::Message(remote));
    }

    /// The bridge model of one TL message, with its media downloaded. `None` for empty messages.
    pub(crate) async fn remote_message(
        &self,
        message: &enums::Message,
        live: bool,
        edit: bool,
    ) -> Option<RemoteMessage> {
        match message {
            enums::Message::Message(m) => {
                let entities = m.entities.clone().unwrap_or_default();
                let body = convert::body(&m.message, &entities, m.media.as_deref());
                let reply = m.reply_to.as_deref().and_then(convert::reply_target);
                let (kind, id) = convert::peer_parts(&m.peer_id);
                let from = m.from_id.as_deref().map(convert::peer_parts);
                let sender = match (m.out, from) {
                    (true, _) => self.me_user(),
                    (false, Some((PeerKind::User, user))) => peers::remote_user(&self.store, user),
                    (false, Some((PeerKind::Channel, channel))) => {
                        peers::remote_channel_user(&self.store, channel)
                    }
                    (false, _) if kind == PeerKind::User => peers::remote_user(&self.store, id),
                    (false, _) if kind == PeerKind::Channel => {
                        peers::remote_channel_user(&self.store, id)
                    }
                    (false, _) => RemoteUser::default(),
                };
                let content = self.content_of(body).await;
                let topic = convert::topic_of(m.reply_to.as_deref());
                Some(self.build_message(
                    &m.peer_id,
                    m.id,
                    MessageParts {
                        sender,
                        date: m.date,
                        content,
                        reply,
                        topic,
                        live,
                        from_me: m.out,
                        edit,
                    },
                ))
            }
            enums::Message::MessageService(s) => {
                let from = s.from_id.as_deref().map(convert::peer_parts);
                let who = match from {
                    Some((PeerKind::User, user)) => peers::display_name_of(&self.store, user),
                    _ => String::new(),
                };
                let text = convert::service_text(&s.action, &who, |id| {
                    peers::display_name_of(&self.store, id)
                });
                let sender = match from {
                    Some((PeerKind::User, user)) => peers::remote_user(&self.store, user),
                    _ => RemoteUser::default(),
                };
                let topic = convert::topic_of(s.reply_to.as_deref());
                Some(self.build_message(
                    &s.peer_id,
                    s.id,
                    MessageParts {
                        sender,
                        date: s.date,
                        content: Content::Notice { body: text },
                        reply: None,
                        topic,
                        live,
                        from_me: s.out,
                        edit: false,
                    },
                ))
            }
            enums::Message::MessageEmpty(_) => None,
        }
    }

    async fn content_of(&self, body: Body) -> Content {
        match body {
            Body::Ready(content) => content,
            Body::Download(download) => match crate::media::download(&self.api, &download).await {
                Ok(data) => Content::Media {
                    kind: download.kind,
                    caption: download.caption,
                    info: download.info,
                    data: Arc::new(data),
                    thumbnail: None,
                },
                Err(error) => {
                    log::debug!("telegram: media download failed: {error}");
                    Content::Unsupported {
                        body: "this file could not be downloaded; open Telegram to view it".into(),
                    }
                }
            },
        }
    }

    /// Assemble the remote message of a peer and id, and remember where its id was seen.
    fn build_message(&self, peer: &enums::Peer, id: i32, parts: MessageParts) -> RemoteMessage {
        let (kind, peer_id) = convert::peer_parts(peer);
        let channel = (kind == PeerKind::Channel).then_some(peer_id);
        let portal = ids::portal(kind, peer_id, parts.topic, &self.login_id());
        let msg_id = ids::message_id(channel, id);
        let reply_to = parts.reply.map(|target| {
            let reply_channel = match target.peer {
                Some((PeerKind::Channel, channel)) => Some(channel),
                Some(_) => None,
                None => channel,
            };
            ids::message_id(reply_channel, target.message)
        });
        let edit_of = parts.edit.then(|| msg_id.clone());
        RemoteMessage {
            portal,
            id: msg_id,
            part: String::new(),
            sender: parts.sender,
            timestamp_ms: i64::from(parts.date) * 1000,
            content: parts.content,
            reply_to,
            edit_of,
            live: parts.live,
            from_me: parts.from_me,
        }
    }

    /// Fetch the difference until the server says it is complete, applying every update in it.
    /// The position moves only once an answer is applied, so a dropped future repeats the request.
    async fn sync_difference(&mut self) -> Result<()> {
        loop {
            let Some(pos) = self.store.position() else {
                self.fetch_state().await?;
                return Ok(());
            };
            let request = functions::updates::GetDifference {
                pts: pos.pts,
                pts_limit: None,
                pts_total_limit: None,
                date: pos.date,
                qts: pos.qts,
                qts_limit: None,
            };
            let answer = self.api.call(&request).await?;
            match answer {
                enums::updates::Difference::DifferenceEmpty(d) => {
                    let mut next = pos;
                    advance_seq(&mut next, d.seq, d.date);
                    self.store.set_position(next);
                    return Ok(());
                }
                enums::updates::Difference::Difference(d) => {
                    self.absorb(&d.users, &d.chats);
                    self.apply_difference(d.new_messages, d.other_updates)
                        .await?;
                    self.store_state(&d.state);
                    return Ok(());
                }
                enums::updates::Difference::DifferenceSlice(d) => {
                    self.absorb(&d.users, &d.chats);
                    self.apply_difference(d.new_messages, d.other_updates)
                        .await?;
                    self.store_state(&d.intermediate_state);
                }
                enums::updates::Difference::DifferenceTooLong(_) => {
                    log::debug!("telegram: difference too long, resetting the update state");
                    self.store.reset_updates();
                    self.fetch_state().await?;
                    return Ok(());
                }
            }
        }
    }

    async fn apply_difference(
        &mut self,
        messages: Vec<enums::Message>,
        updates: Vec<enums::Update>,
    ) -> Result<()> {
        for message in &messages {
            self.emit_message(message, true, false).await;
        }
        for update in updates {
            self.process(update, true, false).await?;
        }
        Ok(())
    }

    fn store_state(&self, state: &enums::updates::State) {
        let enums::updates::State::State(state) = state;
        self.store.set_position(Position {
            pts: state.pts,
            qts: state.qts,
            date: state.date,
            seq: state.seq,
        });
    }

    /// Read the update state from the server (first connect, or after a reset).
    async fn fetch_state(&mut self) -> Result<()> {
        let state = self.api.call(&functions::updates::GetState {}).await?;
        self.store_state(&state);
        Ok(())
    }

    /// Fetch the difference of one channel until it is final.
    async fn channel_difference(&mut self, channel: i64) -> Result<()> {
        loop {
            let Some(local) = self.store.channel_pts(channel) else {
                return Ok(());
            };
            let input = match peers::input_channel(&self.store, channel) {
                Ok(input) => input,
                Err(_) => {
                    self.store.forget_channel(channel);
                    return Ok(());
                }
            };
            let request = functions::updates::GetChannelDifference {
                force: false,
                channel: Box::new(input),
                filter: Box::new(enums::ChannelMessagesFilter::ChannelMessagesFilterEmpty(
                    types::ChannelMessagesFilterEmpty {},
                )),
                pts: local,
                limit: CHANNEL_DIFFERENCE_LIMIT,
            };
            let answer = self.api.call(&request).await?;
            match answer {
                enums::updates::ChannelDifference::ChannelDifferenceEmpty(d) => {
                    self.store.set_channel_pts(channel, d.pts);
                    return Ok(());
                }
                enums::updates::ChannelDifference::ChannelDifferenceTooLong(d) => {
                    self.absorb(&d.users, &d.chats);
                    let pts = match &*d.dialog {
                        enums::Dialog::Dialog(dialog) => dialog.pts.unwrap_or(local),
                        _ => local,
                    };
                    for message in &d.messages {
                        self.emit_message(message, true, false).await;
                    }
                    self.store.set_channel_pts(channel, pts);
                    return Ok(());
                }
                enums::updates::ChannelDifference::ChannelDifference(d) => {
                    self.absorb(&d.users, &d.chats);
                    for message in &d.new_messages {
                        self.emit_message(message, true, false).await;
                    }
                    for update in d.other_updates {
                        self.process(update, true, false).await?;
                    }
                    self.store.set_channel_pts(channel, d.pts);
                    if d.r#final || d.pts <= local {
                        return Ok(());
                    }
                }
            }
        }
    }

    /// Fetch one page of dialogs. The request is the only await; the page is applied and the cursor
    /// moved in the same step, so a dropped future repeats the page and loses nothing.
    async fn dialog_page(&mut self, cursor: DialogCursor) -> Result<()> {
        let request = functions::messages::GetDialogs {
            exclude_pinned: false,
            folder_id: Some(cursor.folder),
            offset_date: cursor.offset_date,
            offset_id: cursor.offset_id,
            offset_peer: Box::new(cursor.offset_peer.clone()),
            limit: DIALOG_PAGE,
            hash: 0,
        };
        let answer = self.api.call(&request).await.map_err(Error::from)?;
        let (dialogs, messages, chats, users, complete) = match answer {
            enums::messages::Dialogs::Dialogs(d) => (d.dialogs, d.messages, d.chats, d.users, true),
            enums::messages::Dialogs::DialogsSlice(d) => {
                (d.dialogs, d.messages, d.chats, d.users, false)
            }
            enums::messages::Dialogs::DialogsNotModified(_) => {
                (Vec::new(), Vec::new(), Vec::new(), Vec::new(), true)
            }
        };
        self.absorb(&users, &chats);
        let count = dialogs.len();
        let last = dialogs.last().cloned();
        for dialog in &dialogs {
            self.emit_dialog(dialog, &users, &chats);
        }
        let following = if complete || count < usize::try_from(DIALOG_PAGE).unwrap_or(0) {
            None
        } else {
            self.following_cursor(last, &messages)
        };
        self.sync = match (following, cursor.folder) {
            (Some((offset_date, offset_id, offset_peer)), folder) => Sync::Dialogs(DialogCursor {
                folder,
                offset_date,
                offset_id,
                offset_peer,
            }),
            (None, 0) => Sync::Dialogs(DialogCursor::first(1)),
            (None, _) => {
                self.finish_sync();
                return Ok(());
            }
        };
        Ok(())
    }

    /// The offset of the page after one whose last dialog is `last`: the top message's date and id,
    /// and the last dialog's peer. `None` when the page cannot be continued.
    fn following_cursor(
        &self,
        last: Option<enums::Dialog>,
        messages: &[enums::Message],
    ) -> Option<(i32, i32, enums::InputPeer)> {
        let Some(enums::Dialog::Dialog(last)) = last else {
            return None;
        };
        if last.top_message == 0 {
            return None;
        }
        let Some(enums::Message::Message(top)) = messages
            .iter()
            .find(|m| message_id_of(m) == Some(last.top_message))
        else {
            return None;
        };
        let peer = self.input_peer_of(&last.peer)?;
        Some((top.date, last.top_message, peer))
    }

    /// The input peer of a dialog's peer, from the cache.
    fn input_peer_of(&self, peer: &enums::Peer) -> Option<enums::InputPeer> {
        let (kind, id) = convert::peer_parts(peer);
        crate::peers::input_peer(&self.store, &ids::Portal { kind, id, topic: 0 }).ok()
    }

    /// Queue the chat of one dialog.
    fn emit_dialog(
        &mut self,
        dialog: &enums::Dialog,
        users: &[enums::User],
        chats: &[enums::Chat],
    ) {
        let enums::Dialog::Dialog(dialog) = dialog else {
            return;
        };
        let (kind, id) = convert::peer_parts(&dialog.peer);
        let login = self.login_id();
        if kind == PeerKind::Channel
            && let Some(pts) = dialog.pts
        {
            self.store.set_channel_pts(id, pts);
        }
        let meta = match kind {
            PeerKind::User => {
                let Some(user) = users.iter().find_map(|u| match u {
                    enums::User::User(user) if user.id == id => Some(user),
                    _ => None,
                }) else {
                    return;
                };
                let remote = RemoteUser {
                    id: ids::user_id(id),
                    display_name: peers::user_name(user),
                    username: user.username.clone().unwrap_or_default(),
                    ..RemoteUser::default()
                };
                ChatMeta {
                    portal: ids::portal(kind, id, 0, &login),
                    name: remote.display_name.clone(),
                    is_dm: true,
                    dm_user: Some(remote),
                    ..ChatMeta::default()
                }
            }
            PeerKind::Chat | PeerKind::Channel => {
                let Some((title, members)) = chats.iter().find_map(|c| chat_title(c, id)) else {
                    return;
                };
                ChatMeta {
                    portal: ids::portal(kind, id, 0, &login),
                    name: title,
                    member_count: i64::from(members),
                    ..ChatMeta::default()
                }
            }
        };
        self.pending.push_back(Event::Chat(meta));
    }
}

/// The channel of a message's peer, when it is one.
fn channel_of_message(message: &enums::Message) -> Option<i64> {
    let peer = match message {
        enums::Message::Message(m) => &m.peer_id,
        enums::Message::MessageService(s) => &s.peer_id,
        enums::Message::MessageEmpty(_) => return None,
    };
    match peer.as_ref() {
        enums::Peer::PeerChannel(p) => Some(p.channel_id),
        _ => None,
    }
}

fn message_id_of(message: &enums::Message) -> Option<i32> {
    match message {
        enums::Message::Message(m) => Some(m.id),
        enums::Message::MessageService(s) => Some(s.id),
        enums::Message::MessageEmpty(m) => Some(m.id),
    }
}

/// The title and member count of a chat or channel object, when it is usable.
fn chat_title(chat: &enums::Chat, id: i64) -> Option<(String, i32)> {
    match chat {
        enums::Chat::Chat(c) if c.id == id => Some((c.title.clone(), c.participants_count)),
        enums::Chat::Channel(c) if c.id == id => {
            Some((c.title.clone(), c.participants_count.unwrap_or(0)))
        }
        _ => None,
    }
}

fn is_cancel(action: &enums::SendMessageAction) -> bool {
    matches!(action, enums::SendMessageAction::SendMessageCancelAction(_))
}

/// The account's own user object.
async fn fetch_self<A: Api>(api: &A) -> Result<types::User> {
    let request = functions::users::GetUsers {
        id: vec![enums::InputUser::InputUserSelf(types::InputUserSelf {})],
    };
    let users = api.call(&request).await.map_err(Error::from)?;
    users
        .into_iter()
        .find_map(|user| match user {
            enums::User::User(user) => Some(user),
            enums::User::UserEmpty(_) => None,
        })
        .ok_or_else(|| Error::Protocol("Telegram returned no account".into()))
}

/// Delivery verdict for an accepted send.
pub(crate) fn sent(portal: PortalRef, txn_id: String) -> Event {
    Event::Delivery {
        portal,
        txn_id,
        status: DeliveryStatus::Sent,
        error: String::new(),
    }
}

#[cfg(test)]
mod tests;
