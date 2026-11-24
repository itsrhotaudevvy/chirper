//! One live login. `run` syncs the chat list, then polls the event log and feeds the runtime;
//! [`BlueskySession`] receives those events and answers the commands and history requests.
//!
//! The poll loop waits 5 seconds between polls and doubles the wait after each empty poll, up to
//! 30 seconds; any entry resets it. A transient failure is reported and retried on the same schedule.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use log::debug;
use serde_json::Value;
use tokio::sync::mpsc;

use chirper_bridge::model::{
    Ack, BackfillPage, PortalRef, RemoteEventRef, RemoteMessage, RemoteUser, SessionState,
};
use chirper_bridge::{Command, Error, Event, Result, Session};

use crate::account::Account;
use crate::api::chat::{self, Log, Message};
use crate::api::identity;
use crate::api::server;
use crate::api::types::MessageView;
use crate::convert::{self, SpanKind, Viewer};
use crate::ids;

/// Conversations read when a login starts, at most.
const MAX_CHATS: usize = 500;
/// The page size the chat service accepts for both lists.
const PAGE: usize = 100;
/// The poll interval, and the longest wait when nothing arrives.
const POLL_SECS: u64 = 5;
const MAX_IDLE_SECS: u64 = 30;

/// The wait before the next poll: the interval, doubled once per empty poll up to the maximum.
fn poll_delay(idle: u32) -> Duration {
    let secs = POLL_SECS
        .saturating_mul(1u64 << idle.min(3))
        .min(MAX_IDLE_SECS);
    Duration::from_secs(secs)
}

fn state(state: SessionState, error: String, profile: Option<RemoteUser>) -> Event {
    Event::State {
        state,
        error,
        message: String::new(),
        profile,
    }
}

/// The background half of a login: sync, then poll until the session is dropped or fails.
pub async fn run(account: Arc<Account>, tx: mpsc::UnboundedSender<Event>, me: RemoteUser) {
    if let Err(err) = run_inner(&account, &tx, me).await {
        let _ = tx.send(state(err.session_state(), err.to_string(), None));
    }
}

async fn run_inner(
    account: &Account,
    tx: &mpsc::UnboundedSender<Event>,
    me: RemoteUser,
) -> Result<()> {
    if !sync_chats(account, tx).await? {
        return Ok(());
    }
    if tx
        .send(state(SessionState::Running, String::new(), Some(me)))
        .is_err()
    {
        return Ok(());
    }
    poll(account, tx).await
}

/// Send every conversation as chat metadata, newest first. Returns `false` when the session was
/// dropped meanwhile.
async fn sync_chats(account: &Account, tx: &mpsc::UnboundedSender<Event>) -> Result<bool> {
    let mut cursor: Option<String> = None;
    let mut sent = 0usize;
    loop {
        let cursor_ref = cursor.as_deref();
        let page = account
            .send(|pds, token| async move { chat::list_convos(&pds, &token, cursor_ref).await })
            .await?;
        for convo in &page.convos {
            for member in &convo.members {
                account.remember(member);
            }
            match convert::chat_meta(convo, account.did()) {
                Ok(meta) => {
                    if tx.send(Event::Chat(meta)).is_err() {
                        return Ok(false);
                    }
                    sent += 1;
                }
                Err(err) => debug!("bluesky: skipped a conversation: {err}"),
            }
        }
        match page.cursor {
            Some(next) if sent < MAX_CHATS && !page.convos.is_empty() => cursor = Some(next),
            _ => return Ok(true),
        }
    }
}

/// Poll the event log until the session is dropped (`Ok`) or a failure ends it (`Err`).
async fn poll(account: &Account, tx: &mpsc::UnboundedSender<Event>) -> Result<()> {
    let mut erroring = false;
    let mut idle = 0u32;
    loop {
        let mut wait = poll_delay(idle);
        match poll_once(account, tx).await {
            Ok(None) => return Ok(()),
            Ok(Some(entries)) => {
                if erroring {
                    erroring = false;
                    if tx
                        .send(state(SessionState::Running, String::new(), None))
                        .is_err()
                    {
                        return Ok(());
                    }
                }
                idle = if entries == 0 {
                    idle.saturating_add(1)
                } else {
                    0
                };
            }
            Err(err) if err.is_retryable() => {
                debug!("bluesky: poll failed: {err}");
                if let Error::RateLimited { retry_after_ms } = err {
                    wait = wait.max(Duration::from_millis(retry_after_ms));
                }
                erroring = true;
                idle = 0;
                if tx
                    .send(state(
                        SessionState::TransientDisconnect,
                        err.to_string(),
                        None,
                    ))
                    .is_err()
                {
                    return Ok(());
                }
            }
            Err(err) => return Err(err),
        }
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = tx.closed() => return Ok(()),
        }
    }
}

/// One poll: fetch the entries after the saved cursor, hand their events on, then save the cursor.
/// `None` when the session was dropped; otherwise the number of entries read.
async fn poll_once(account: &Account, tx: &mpsc::UnboundedSender<Event>) -> Result<Option<usize>> {
    let cursor = account.cursor().await;
    let cursor_ref = cursor.as_deref();
    let page = account
        .send(|pds, token| async move { chat::get_log(&pds, &token, cursor_ref).await })
        .await?;
    let entries = page.logs.len();
    let logs: Vec<Log> = page.logs.into_iter().filter_map(parse_entry).collect();
    let dids: Vec<String> = logs.iter().flat_map(Log::dids).collect();
    let users = account.users(&dids).await?;
    let viewer = Viewer {
        did: account.did(),
        users: &users,
    };
    for log in &logs {
        for event in log_events(log, &viewer) {
            if tx.send(event).is_err() {
                return Ok(None);
            }
        }
    }
    if let Some(next) = page.cursor
        && cursor.as_deref() != Some(next.as_str())
    {
        account.set_cursor(next).await?;
    }
    Ok(Some(entries))
}

fn parse_entry(entry: Value) -> Option<Log> {
    match chat::parse_log(entry) {
        Ok(log) => Some(log),
        Err(err) => {
            debug!("bluesky: skipped a log entry: {err}");
            None
        }
    }
}

/// The runtime events of one log entry. An entry that cannot be converted is skipped.
fn log_events(log: &Log, viewer: &Viewer<'_>) -> Vec<Event> {
    match checked_events(log, viewer) {
        Ok(events) => events,
        Err(err) => {
            debug!("bluesky: skipped an event: {err}");
            Vec::new()
        }
    }
}

fn checked_events(log: &Log, viewer: &Viewer<'_>) -> Result<Vec<Event>> {
    match log {
        Log::Created {
            convo,
            message: Message::View(view),
        } => {
            let mut events = vec![Event::Message(convert::text_message(
                convo, view, viewer, true,
            )?)];
            if !view.reactions.is_empty() {
                events.push(reactions_event(convo, view, viewer)?);
            }
            Ok(events)
        }
        // A deleted message is gone for everyone who receives the entry.
        Log::Created { convo, message } | Log::Deleted { convo, message } => Ok(message
            .id()
            .map(|id| Event::Redact {
                portal: convert::portal(convo, viewer.did),
                event: RemoteEventRef::new(ids::message_id(convo, id)),
            })
            .into_iter()
            .collect()),
        Log::Reactions { convo, message } => Ok(vec![reactions_event(convo, message, viewer)?]),
        Log::Other => Ok(Vec::new()),
    }
}

fn reactions_event(convo: &str, view: &MessageView, viewer: &Viewer<'_>) -> Result<Event> {
    Ok(Event::Reactions {
        portal: convert::portal(convo, viewer.did),
        target: RemoteEventRef::new(ids::message_id(convo, &view.id)),
        reactions: convert::reactions(view, viewer)?,
    })
}

/// The live session the runtime drives: events from the poll loop, commands over XRPC.
pub struct BlueskySession {
    account: Arc<Account>,
    me: RemoteUser,
    events: mpsc::UnboundedReceiver<Event>,
}

impl BlueskySession {
    pub fn new(
        account: Arc<Account>,
        me: RemoteUser,
        events: mpsc::UnboundedReceiver<Event>,
    ) -> Self {
        Self {
            account,
            me,
            events,
        }
    }

    async fn send_text(
        &self,
        portal: &PortalRef,
        text: &str,
        reply_to: Option<&str>,
    ) -> Result<Ack> {
        if reply_to.is_some() {
            return Err(Error::Unsupported("replies on Bluesky"));
        }
        let facets = self.facets_for(text).await;
        let convo = portal.id.as_str();
        let facets = facets.as_slice();
        let sent = self
            .account
            .send(|pds, token| async move {
                chat::send_message(&pds, &token, convo, text, facets).await
            })
            .await?;
        Ok(Ack {
            remote_event_id: Some(ids::message_id(&portal.id, &sent.id)),
        })
    }

    /// The facets of a draft: its web links, and its mentions resolved to DIDs. A mention that does
    /// not resolve is sent as plain text.
    async fn facets_for(&self, text: &str) -> Vec<Value> {
        let mut facets = Vec::new();
        for span in convert::detect(text) {
            match span.kind {
                SpanKind::Link(uri) => facets.push(convert::facet_json(
                    span.start,
                    span.end,
                    convert::link_feature(&uri),
                )),
                SpanKind::Mention(handle) => {
                    if let Ok(did) = self.resolve(&handle).await {
                        facets.push(convert::facet_json(
                            span.start,
                            span.end,
                            convert::mention_feature(&did),
                        ));
                    }
                }
            }
        }
        facets
    }

    async fn resolve(&self, handle: &str) -> Result<String> {
        self.account
            .send(|pds, _token| async move { identity::resolve_handle(&pds, handle).await })
            .await
    }
}

#[async_trait]
impl Session for BlueskySession {
    async fn next(&mut self) -> Option<Event> {
        self.events.recv().await
    }

    async fn command(&mut self, command: Command) -> Result<Ack> {
        match command {
            Command::SendText {
                portal,
                text,
                reply_to,
                txn_id: _,
            } => self.send_text(&portal, &text, reply_to.as_deref()).await,
            Command::SendMedia { .. } => Err(Error::Unsupported("sending media to Bluesky")),
            Command::Edit { .. } => Err(Error::Unsupported("editing messages on Bluesky")),
            Command::Redact { portal, target } => {
                let message = ids::message_in(&portal.id, &target)?;
                let convo = portal.id.as_str();
                self.account
                    .send(|pds, token| async move {
                        chat::delete_for_self(&pds, &token, convo, message).await
                    })
                    .await?;
                Ok(Ack::default())
            }
            Command::React {
                portal,
                target,
                key,
            } => {
                let message = ids::message_in(&portal.id, &target)?;
                let convo = portal.id.as_str();
                let key = key.as_str();
                self.account
                    .send(|pds, token| async move {
                        chat::add_reaction(&pds, &token, convo, message, key).await
                    })
                    .await?;
                Ok(Ack::default())
            }
            Command::Unreact {
                portal,
                target,
                key,
            } => {
                let message = ids::message_in(&portal.id, &target)?;
                let convo = portal.id.as_str();
                let key = key.as_str();
                self.account
                    .send(|pds, token| async move {
                        chat::remove_reaction(&pds, &token, convo, message, key).await
                    })
                    .await?;
                Ok(Ack::default())
            }
            Command::Read { portal, event } => {
                let message = event
                    .as_deref()
                    .map(|target| ids::message_in(&portal.id, target))
                    .transpose()?;
                let convo = portal.id.as_str();
                self.account
                    .send(|pds, token| async move {
                        chat::update_read(&pds, &token, convo, message).await
                    })
                    .await?;
                Ok(Ack::default())
            }
            Command::Typing { .. } => Err(Error::Unsupported("typing indicators on Bluesky")),
        }
    }

    async fn backfill(
        &mut self,
        portal: &PortalRef,
        cursor: Option<String>,
        limit: usize,
    ) -> Result<BackfillPage> {
        let limit = limit.clamp(1, PAGE);
        let cursor_ref = cursor.as_deref();
        let convo = portal.id.as_str();
        let page = self
            .account
            .send(|pds, token| async move {
                chat::get_messages(&pds, &token, convo, limit, cursor_ref).await
            })
            .await?;
        let raw = page.messages.len();
        let messages: Vec<Message> = page
            .messages
            .into_iter()
            .filter_map(|entry| match chat::parse_message(entry) {
                Ok(message) => Some(message),
                Err(err) => {
                    debug!("bluesky: skipped a history entry: {err}");
                    None
                }
            })
            .collect();
        let dids: Vec<String> = messages.iter().flat_map(Message::dids).collect();
        let users = self.account.users(&dids).await?;
        let viewer = Viewer {
            did: self.account.did(),
            users: &users,
        };
        let mut history: Vec<RemoteMessage> = messages
            .iter()
            .filter_map(
                |message| match convert::history_message(&portal.id, message, &viewer) {
                    Ok(converted) => converted,
                    Err(err) => {
                        debug!("bluesky: skipped a history message: {err}");
                        None
                    }
                },
            )
            .collect();
        // The service lists newest first; the runtime wants oldest first within a page.
        history.reverse();
        Ok(BackfillPage {
            messages: history,
            has_more: page.cursor.is_some() && raw >= limit,
            cursor: page.cursor,
        })
    }

    async fn logout_remote(&mut self) -> Result<()> {
        self.account
            .send(|pds, token| async move { server::delete_session(&pds, &token).await })
            .await
    }

    fn user(&self) -> Option<RemoteUser> {
        Some(self.me.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_poll_wait_doubles_when_idle_and_stops_at_thirty_seconds() {
        let secs: Vec<u64> = (0..6).map(|idle| poll_delay(idle).as_secs()).collect();
        assert_eq!(secs, vec![5, 10, 20, 30, 30, 30]);
    }
}
