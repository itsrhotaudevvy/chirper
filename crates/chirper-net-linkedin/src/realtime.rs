//! The realtime stream: a server-sent-events connection with a heartbeat, reconnected with
//! backoff, and its events turned into the bridge's events.
//! Reference: `linkedingo/realtime.go` and `connector/handlelinkedin.go`.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use log::debug;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

use chirper_bridge::model::{Reaction, RemoteEventRef, SessionState};
use chirper_bridge::{Error, Event};

use crate::api::Api;
use crate::api::model::{DecoratedEvent, Message, RealtimeLine};
use crate::api::uuid_v4;
use crate::convert;
use crate::ids;
use crate::session::{self, Shared, now_ms, portal_of};

/// The first reconnect waits this long, and each failed attempt doubles the wait.
const MIN_BACKOFF: Duration = Duration::from_secs(2);
/// The longest wait between reconnects.
const MAX_BACKOFF: Duration = Duration::from_secs(300);
/// A stream that stayed up this long resets the backoff.
const STABLE_UPTIME: Duration = Duration::from_secs(60);
/// Heartbeats are sent this often.
const HEARTBEAT_EVERY: Duration = Duration::from_secs(60);
/// Two rejected heartbeats within this window mean the session is gone.
const REJECTION_WINDOW: Duration = Duration::from_secs(300);
/// Delivered messages remembered so each is acknowledged once.
const ACKED_CACHE: usize = 1000;

/// What the stream task needs from its login.
pub struct Link {
    pub api: Api,
    pub me_id: String,
    pub shared: Arc<Shared>,
    pub tx: UnboundedSender<Event>,
}

/// How one connection ended.
enum Ended {
    /// The server closed the stream. Reconnect.
    Closed,
    /// The heartbeat was refused. Reconnect with a new stream id, and give up if it repeats.
    HeartbeatRejected,
    /// Something failed; the error decides whether the login is over.
    Failed(Error),
}

/// Keep the stream connected until the login ends.
pub async fn run(link: Link) {
    let mut backoff = MIN_BACKOFF;
    let mut session_id = uuid_v4();
    let mut rejected_at: Option<Instant> = None;
    let mut acked: HashSet<String> = HashSet::new();
    let mut connected_before = false;
    loop {
        if link.tx.is_closed() {
            return;
        }
        let started = Instant::now();
        let ended = stream(&link, &session_id, connected_before, &mut acked).await;
        connected_before = true;
        match ended {
            Ended::HeartbeatRejected => {
                if rejected_at.is_some_and(|at| at.elapsed() < REJECTION_WINDOW) {
                    report_reauth(&link, "LinkedIn rejected the realtime heartbeat");
                    return;
                }
                rejected_at = Some(Instant::now());
                session_id = uuid_v4();
                transient(&link, "Reconnecting to LinkedIn");
            }
            Ended::Failed(error) if error.session_state() == SessionState::NeedsReauth => {
                report_reauth(&link, &error.to_string());
                return;
            }
            Ended::Failed(error) => {
                debug!("linkedin: realtime stream failed: {error}");
                transient(&link, "Lost connection to LinkedIn, reconnecting");
            }
            Ended::Closed => transient(&link, "The LinkedIn stream closed, reconnecting"),
        }
        if started.elapsed() >= STABLE_UPTIME {
            backoff = MIN_BACKOFF;
        }
        tokio::time::sleep(backoff / 2 + jitter(backoff / 2)).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

fn transient(link: &Link, message: &str) {
    session::send(
        &link.tx,
        session::state(SessionState::TransientDisconnect, "", message, None),
    );
}

fn report_reauth(link: &Link, error: &str) {
    session::send(
        &link.tx,
        session::state(
            SessionState::NeedsReauth,
            error,
            "Logged out of LinkedIn, please log in again",
            None,
        ),
    );
}

/// A random delay below `bound`, so reconnecting clients do not synchronise.
fn jitter(bound: Duration) -> Duration {
    let millis = u64::try_from(bound.as_millis()).unwrap_or(0);
    if millis == 0 {
        return Duration::ZERO;
    }
    Duration::from_millis(rand::random::<u64>() % millis)
}

/// One connection: open the stream, resync the chats after a reconnect, then read until it ends.
async fn stream(link: &Link, session_id: &str, resync: bool, acked: &mut HashSet<String>) -> Ended {
    let mut response = match link.api.open_stream(session_id).await {
        Ok(response) => response,
        Err(error) => return Ended::Failed(error),
    };
    if resync
        && session::sync_chats(&link.api, &link.me_id, &link.shared, &link.tx)
            .await
            .is_err()
    {
        debug!("linkedin: resync after reconnect failed");
    }
    let actor = ids::profile_urn(&link.me_id);
    let mut heartbeat: JoinHandle<Error> =
        tokio::spawn(heartbeats(link.api.clone(), session_id.to_owned(), actor));
    let mut buffer: Vec<u8> = Vec::new();
    loop {
        tokio::select! {
            chunk = response.chunk() => match chunk {
                Ok(Some(bytes)) => {
                    buffer.extend_from_slice(&bytes);
                    while let Some(position) = buffer.iter().position(|byte| *byte == b'\n') {
                        let line: Vec<u8> = buffer.drain(..=position).collect();
                        if !handle_line(link, &line, acked).await {
                            heartbeat.abort();
                            return Ended::Closed;
                        }
                    }
                }
                Ok(None) => {
                    heartbeat.abort();
                    return Ended::Closed;
                }
                Err(_) => {
                    heartbeat.abort();
                    return Ended::Failed(Error::Network("LinkedIn: the realtime stream was interrupted".to_owned()));
                }
            },
            outcome = &mut heartbeat => {
                return match outcome {
                    Ok(Error::Auth(_)) => Ended::HeartbeatRejected,
                    Ok(error) => Ended::Failed(error),
                    Err(_) => Ended::Failed(Error::Internal("the heartbeat task stopped".to_owned())),
                };
            }
        }
    }
}

/// Send heartbeats until one fails. The error is what ends the stream.
async fn heartbeats(api: Api, session_id: String, actor: String) -> Error {
    let mut first = true;
    loop {
        if let Err(error) = api.heartbeat(&session_id, &actor, first).await {
            return error;
        }
        first = false;
        tokio::time::sleep(HEARTBEAT_EVERY).await;
    }
}

/// Handle one line of the stream. Returns `false` once the login's receiver is gone.
async fn handle_line(link: &Link, line: &[u8], acked: &mut HashSet<String>) -> bool {
    let Some(payload) = data_of(line) else {
        return true;
    };
    let Ok(parsed) = serde_json::from_slice::<RealtimeLine>(payload) else {
        debug!("linkedin: skipped a realtime line that did not parse");
        return true;
    };
    let Some(decorated) = parsed.decorated else {
        return true;
    };
    for event in decorated_events(link, decorated, acked).await {
        if !session::send(&link.tx, event) {
            return false;
        }
    }
    true
}

/// The `data:` payload of an SSE line, without the line ending.
fn data_of(line: &[u8]) -> Option<&[u8]> {
    let trimmed = line.strip_suffix(b"\n").unwrap_or(line);
    let trimmed = trimmed.strip_suffix(b"\r").unwrap_or(trimmed);
    trimmed.strip_prefix(b"data:")
}

/// The bridge events of one decorated realtime event.
async fn decorated_events(
    link: &Link,
    event: DecoratedEvent,
    acked: &mut HashSet<String>,
) -> Vec<Event> {
    let topic = event.topic.split(':').nth(2).unwrap_or("").to_owned();
    let data = event.payload.data;
    let me = link.me_id.as_str();
    let shared = link.shared.as_ref();
    match topic.as_str() {
        "conversationsTopic" | "tabBadgeUpdateTopic" => data
            .conversation
            .and_then(|decorated| session::conversation_event(&decorated.result, me, shared))
            .into_iter()
            .collect(),
        "conversationDeletesTopic" => data
            .conversation_delete
            .map(|decorated| {
                let conversation = decorated.result;
                vec![Event::ChatRemoved(portal_of(
                    shared,
                    me,
                    &conversation.entity_urn,
                    conversation.group_chat,
                ))]
            })
            .unwrap_or_default(),
        "messagesTopic" => match data.message {
            Some(decorated) => message_events(link, decorated.result, acked).await,
            None => Vec::new(),
        },
        "typingIndicatorsTopic" => match data.typing {
            Some(decorated) => {
                let typing = decorated.result;
                vec![Event::Typing {
                    portal: portal_of(
                        shared,
                        me,
                        &typing.conversation.entity_urn,
                        typing.conversation.group_chat,
                    ),
                    senders: vec![convert::remote_user(&typing.participant)],
                }]
            }
            None => Vec::new(),
        },
        "messageSeenReceiptsTopic" => match data.seen {
            Some(decorated) => {
                let seen = decorated.result;
                let conversation = convert::portal_of(&seen.message);
                vec![Event::ReadReceipt {
                    portal: portal_of(
                        shared,
                        me,
                        &conversation,
                        seen.message.conversation.group_chat,
                    ),
                    user: convert::remote_user(&seen.seen_by),
                    event: RemoteEventRef::new(ids::message_id(&seen.message.entity_urn)),
                }]
            }
            None => Vec::new(),
        },
        "messageReactionSummariesTopic" => match data.reaction {
            Some(decorated) => reaction_events(link, decorated.result),
            None => Vec::new(),
        },
        _ => {
            debug!("linkedin: ignored a realtime topic");
            Vec::new()
        }
    }
}

/// Messages, edits and recalls. A delivered message is acknowledged once.
async fn message_events(link: &Link, message: Message, acked: &mut HashSet<String>) -> Vec<Event> {
    if !message.entity_urn.is_empty() && acknowledge(acked, &message.entity_urn) {
        let api = link.api.clone();
        let urn = message.entity_urn.clone();
        tokio::spawn(async move {
            if let Err(error) = api
                .acknowledge_delivery(std::slice::from_ref(&urn), now_ms())
                .await
            {
                debug!("linkedin: a delivery acknowledgement failed: {error}");
            }
        });
    }
    let conversation = convert::portal_of(&message);
    if conversation.is_empty() {
        return Vec::new();
    }
    let portal = portal_of(
        &link.shared,
        &link.me_id,
        &conversation,
        message.conversation.group_chat,
    );
    let target = RemoteEventRef::new(ids::message_id(&message.entity_urn));
    match message.render_format.as_str() {
        "RECALLED" => vec![Event::Redact {
            portal,
            event: target,
        }],
        "SYSTEM" => Vec::new(),
        _ => {
            let group = portal.receiver.is_empty();
            convert::messages(&link.api, &message, group, true, &link.me_id)
                .await
                .into_iter()
                .map(Event::Message)
                .collect()
        }
    }
}

/// Whether `urn` is new to the acknowledged set. The set is cleared when it grows past its bound.
fn acknowledge(acked: &mut HashSet<String>, urn: &str) -> bool {
    if acked.len() >= ACKED_CACHE {
        acked.clear();
    }
    acked.insert(urn.to_owned())
}

/// A reaction change. The stream carries the whole reaction summary of the message only when the
/// message does; without it the full set is unknown, so nothing is reported rather than a wrong set.
fn reaction_events(link: &Link, change: crate::api::model::ReactionChange) -> Vec<Event> {
    if change.message.reaction_summaries.is_empty() {
        debug!("linkedin: a reaction change carried no summary set");
        return Vec::new();
    }
    let conversation = convert::portal_of(&change.message);
    let reactions: Vec<Reaction> = convert::reactions(&change.message.reaction_summaries);
    vec![Event::Reactions {
        portal: portal_of(
            &link.shared,
            &link.me_id,
            &conversation,
            change.message.conversation.group_chat,
        ),
        target: RemoteEventRef::new(ids::message_id(&change.message.entity_urn)),
        reactions,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_lines_yield_their_payload() {
        assert_eq!(data_of(b"data: {\"a\":1}\n"), Some(&b" {\"a\":1}"[..]));
        assert_eq!(data_of(b"data:{}\r\n"), Some(&b"{}"[..]));
        assert_eq!(data_of(b": comment\n"), None);
    }

    #[test]
    fn a_message_is_acknowledged_once() {
        let mut acked = HashSet::new();
        assert!(acknowledge(
            &mut acked,
            "urn:li:msg_message:(urn:li:fsd_profile:A,1)"
        ));
        assert!(!acknowledge(
            &mut acked,
            "urn:li:msg_message:(urn:li:fsd_profile:A,1)"
        ));
    }

    #[test]
    fn the_backoff_jitter_stays_below_its_bound() {
        for _ in 0..50 {
            assert!(jitter(Duration::from_secs(1)) < Duration::from_secs(1));
        }
        assert_eq!(jitter(Duration::ZERO), Duration::ZERO);
    }
}
