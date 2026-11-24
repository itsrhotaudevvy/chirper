//! The live side of one login: the directory sync at connect time, the real-time websocket
//! (`rtm.connect`), keep-alive pings, and the translation of Slack's events into bridge events.
//!
//! The Go bridge's RTM consumer (`connector/client.go`, `connector/handleslack.go`) is the reference
//! for which events matter and what they become. A `goodbye` or a dropped socket ends the task; the
//! runtime reconnects by calling `connect` again.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use log::{debug, warn};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message as Frame;

use chirper_bridge::model::{
    ChatMeta, Event, Member, PortalRef, RemoteEventRef, RemoteUser, SessionState,
};
use chirper_bridge::{Error, Result, redact};

use crate::api::Api;
use crate::convert::{self, Scope};
use crate::directory::{self, Conversation, Directory};
use crate::ids;
use crate::wire::{Channel, Message};

/// How often a ping is sent over the websocket.
const PING_EVERY: Duration = Duration::from_secs(30);
/// Most conversations kept in a sync, newest first.
pub const MAX_CONVERSATIONS: usize = 500;
/// Pages read from each list method during a sync.
const MAX_PAGES: usize = 20;

/// Something the session asks the socket task to send.
#[derive(Debug)]
pub enum Outgoing {
    /// The local user is typing in this channel.
    Typing(String),
}

/// Everything the socket task needs from its login.
pub struct Link {
    pub api: Api,
    pub team: String,
    pub team_name: String,
    /// The local user's Slack id.
    pub me: String,
    pub login_id: String,
    pub me_user: RemoteUser,
    pub directory: Arc<Directory>,
    pub events: mpsc::UnboundedSender<Event>,
}

/// How the connection ended.
enum Ending {
    /// The socket went away; the runtime may reconnect.
    Transient,
    /// The login cannot go on as it is.
    Stop { state: SessionState, error: String },
}

/// Run one connection until it ends. Every outcome is reported as a state event.
pub async fn run(link: Link, mut outgoing: mpsc::UnboundedReceiver<Outgoing>) {
    send(&link, state(SessionState::Connecting, String::new()));
    if let Err(err) = sync(&link).await {
        finish_with(&link, &err);
        return;
    }
    let socket = match open(&link.api).await {
        Ok(socket) => socket,
        Err(err) => {
            finish_with(&link, &err);
            return;
        }
    };
    let ending = drive(&link, socket, &mut outgoing).await;
    match ending {
        Ending::Transient => send(&link, state(SessionState::TransientDisconnect, String::new())),
        Ending::Stop { state: value, error } => send(&link, state(value, error)),
    }
}

fn finish_with(link: &Link, err: &Error) {
    debug!("slack: the login ended: {err}");
    send(
        link,
        state(err.session_state(), redact(&err.to_string())),
    );
}

fn state(value: SessionState, error: String) -> Event {
    Event::State {
        state: value,
        error,
        message: String::new(),
        profile: None,
    }
}

fn send(link: &Link, event: Event) {
    let _ = link.events.send(event);
}

fn scope(link: &Link) -> Scope<'_> {
    Scope {
        team: &link.team,
        me: &link.me,
        login_id: &link.login_id,
        directory: &link.directory,
    }
}

/// Load the members, then every conversation the user can see, newest first.
async fn sync(link: &Link) -> Result<()> {
    let mut cursor = String::new();
    for _ in 0..MAX_PAGES {
        let page = link.api.users_list(&cursor).await?;
        directory::load_members(&link.directory, &link.team, &page.members);
        cursor = page.response_metadata.next_cursor;
        if cursor.is_empty() {
            break;
        }
    }
    send(link, Event::Chat(convert::team_meta(&link.team, &link.team_name)));

    let mut channels: Vec<Channel> = Vec::new();
    let mut cursor = String::new();
    for _ in 0..MAX_PAGES {
        let page = link.api.conversations_list(&cursor).await?;
        channels.extend(page.channels);
        cursor = page.response_metadata.next_cursor;
        if cursor.is_empty() || channels.len() >= MAX_CONVERSATIONS * 2 {
            break;
        }
    }
    channels.retain(|channel| channel.is_im || channel.is_mpim || channel.is_member);
    channels.sort_by_key(|channel| std::cmp::Reverse(recency(channel)));
    channels.truncate(MAX_CONVERSATIONS);
    for channel in &channels {
        emit_conversation(link, channel).await?;
    }
    Ok(())
}

/// The last time a conversation moved, in milliseconds, for ordering the list.
fn recency(channel: &Channel) -> i64 {
    if channel.updated > 0 {
        channel.updated
    } else {
        channel.created.saturating_mul(1000)
    }
}

/// Remember a conversation and report it as a chat.
async fn emit_conversation(link: &Link, channel: &Channel) -> Result<()> {
    let is_dm = channel.is_im || channel.is_mpim;
    link.directory.remember_conversation(
        &channel.id,
        Conversation {
            name: crate::api::channel_label(channel),
            is_dm,
        },
    );
    let partner = if channel.is_im && !channel.user.is_empty() {
        Some(directory::resolve(&link.api, &link.directory, &link.team, &channel.user).await)
    } else {
        None
    };
    let meta: ChatMeta = convert::chat_meta(&scope(link), &link.team_name, channel, partner);
    send(link, Event::Chat(meta));
    Ok(())
}

/// Open the websocket `rtm.connect` names.
async fn open(
    api: &Api,
) -> Result<impl futures_util::Stream<Item = std::result::Result<Frame, tokio_tungstenite::tungstenite::Error>> + futures_util::Sink<Frame> + Unpin> {
    let answer = api.rtm_connect().await?;
    let (socket, _) = tokio_tungstenite::connect_async(answer.url.as_str())
        .await
        .map_err(|err| Error::Network(redact(&err.to_string())))?;
    Ok(socket)
}

/// Pump the socket until it ends: frames in, pings out, typing out.
async fn drive<S>(link: &Link, mut socket: S, outgoing: &mut mpsc::UnboundedReceiver<Outgoing>) -> Ending
where
    S: futures_util::Stream<Item = std::result::Result<Frame, tokio_tungstenite::tungstenite::Error>>
        + futures_util::Sink<Frame>
        + Unpin,
{
    let mut ping = tokio::time::interval_at(Instant::now() + PING_EVERY, PING_EVERY);
    let mut frame_id: u64 = 0;
    loop {
        tokio::select! {
            incoming = socket.next() => match incoming {
                Some(Ok(Frame::Text(text))) => {
                    if let Some(ending) = handle_text(link, text.as_str()).await {
                        return ending;
                    }
                }
                Some(Ok(Frame::Ping(payload))) => {
                    let _ = socket.send(Frame::Pong(payload)).await;
                }
                Some(Ok(Frame::Close(_))) | None => return Ending::Transient,
                Some(Ok(_)) => {}
                Some(Err(err)) => {
                    debug!("slack: the socket failed: {}", redact(&err.to_string()));
                    return Ending::Transient;
                }
            },
            request = outgoing.recv() => match request {
                Some(Outgoing::Typing(channel)) => {
                    frame_id += 1;
                    let text = json!({ "id": frame_id, "type": "typing", "channel": channel }).to_string();
                    if socket.send(Frame::Text(text.into())).await.is_err() {
                        return Ending::Transient;
                    }
                }
                None => return Ending::Transient,
            },
            _ = ping.tick() => {
                frame_id += 1;
                let text = json!({ "id": frame_id, "type": "ping" }).to_string();
                if socket.send(Frame::Text(text.into())).await.is_err() {
                    return Ending::Transient;
                }
            }
        }
    }
}

/// Handle one realtime event. Returns the ending when the event closes the connection.
async fn handle_text(link: &Link, text: &str) -> Option<Ending> {
    let value: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(_) => {
            warn!("slack: a socket frame was not JSON");
            return None;
        }
    };
    match value.get("type").and_then(Value::as_str).unwrap_or_default() {
        "hello" => {
            debug!("slack: the socket is up");
            send(
                link,
                Event::State {
                    state: SessionState::Running,
                    error: String::new(),
                    message: String::new(),
                    profile: Some(link.me_user.clone()),
                },
            );
        }
        "goodbye" => return Some(Ending::Transient),
        "error" => {
            let message = value
                .pointer("/error/msg")
                .and_then(Value::as_str)
                .unwrap_or("unknown socket error");
            let code = value
                .pointer("/error/code")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            // Slack's auth codes are final: reconnecting with the same token would fail the same
            // way, so the login stops and asks for a new sign-in.
            if matches!(code, 1..=4) {
                return Some(Ending::Stop {
                    state: SessionState::NeedsReauth,
                    error: redact(&format!("Slack socket error: {message}")),
                });
            }
            send(link, Event::Error(redact(&format!("Slack socket error: {message}"))));
        }
        "message" => on_message(link, value).await,
        "reaction_added" | "reaction_removed" => on_reaction(link, value).await,
        "user_typing" => on_typing(link, value).await,
        "channel_marked" | "im_marked" | "group_marked" => on_marked(link, value),
        "channel_joined" | "group_joined" | "im_created" => {
            if let Ok(channel) = serde_json::from_value::<Channel>(value["channel"].clone()) {
                let _ = emit_conversation(link, &channel).await;
            }
        }
        "channel_left" | "group_left" => on_self_left(link, value),
        "member_joined_channel" | "member_left_channel" => on_member(link, value).await,
        other => debug!("slack: event {other} is not bridged"),
    }
    None
}

/// Message subtypes the bridge shows. Other subtypes (joins, topic changes, hidden notices) are not
/// messages.
pub fn is_message_subtype(subtype: &str) -> bool {
    matches!(
        subtype,
        "" | "me_message" | "bot_message" | "thread_broadcast" | "file_share"
    )
}

async fn on_message(link: &Link, value: Value) {
    let Ok(message) = serde_json::from_value::<Message>(value) else {
        return;
    };
    if message.channel.is_empty() {
        return;
    }
    remember_kind(link, &message);
    let channel = message.channel.clone();
    match message.subtype.as_str() {
        "message_changed" => {
            let Some(inner) = message.message.as_deref() else {
                return;
            };
            if inner.ts.is_empty() {
                return;
            }
            if let Some(edit) = convert::edit(&link.api, &scope(link), &channel, inner, &message.event_ts).await {
                send(link, Event::Message(edit));
            }
        }
        "message_deleted" => {
            if message.deleted_ts.is_empty() {
                return;
            }
            send(
                link,
                Event::Redact {
                    portal: portal(link, &channel),
                    event: RemoteEventRef::new(ids::message_id(&link.team, &channel, &message.deleted_ts)),
                },
            );
        }
        subtype if is_message_subtype(subtype) => {
            let parts = convert::messages(&link.api, &scope(link), &channel, &message, true).await;
            for part in parts {
                send(link, Event::Message(part));
            }
        }
        other => debug!("slack: message subtype {other} is not bridged"),
    }
}

/// Learn whether a conversation is a DM from the type a message names.
fn remember_kind(link: &Link, message: &Message) {
    if message.channel_type.is_empty() || link.directory.conversation(&message.channel).is_some() {
        return;
    }
    let is_dm = matches!(message.channel_type.as_str(), "im" | "mpim");
    link.directory.remember_conversation(
        &message.channel,
        Conversation {
            name: String::new(),
            is_dm,
        },
    );
}

fn portal(link: &Link, channel: &str) -> PortalRef {
    scope(link).portal(channel)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ReactionEvent {
    user: String,
    reaction: String,
    item: ReactionItem,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ReactionItem {
    channel: String,
    ts: String,
}

async fn on_reaction(link: &Link, value: Value) {
    let Ok(event) = serde_json::from_value::<ReactionEvent>(value) else {
        return;
    };
    if event.item.channel.is_empty() || event.item.ts.is_empty() || event.reaction.is_empty() {
        return;
    }
    // Slack reports one reactor per event; the bridge wants the whole set, so read the message.
    match link.api.reactions_get(&event.item.channel, &event.item.ts).await {
        Ok(answer) => send(
            link,
            Event::Reactions {
                portal: portal(link, &event.item.channel),
                target: RemoteEventRef::new(ids::message_id(&link.team, &event.item.channel, &event.item.ts)),
                reactions: convert::reactions(&answer.message.reactions, &link.me),
            },
        ),
        Err(err) => debug!("slack: a reaction could not be read: {err}"),
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct TypingEvent {
    channel: String,
    user: String,
}

async fn on_typing(link: &Link, value: Value) {
    let Ok(event) = serde_json::from_value::<TypingEvent>(value) else {
        return;
    };
    if event.channel.is_empty() || event.user.is_empty() {
        return;
    }
    let sender = directory::resolve(&link.api, &link.directory, &link.team, &event.user).await;
    send(
        link,
        Event::Typing {
            portal: portal(link, &event.channel),
            senders: vec![sender],
        },
    );
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct MarkedEvent {
    channel: String,
    ts: String,
}

fn on_marked(link: &Link, value: Value) {
    let Ok(event) = serde_json::from_value::<MarkedEvent>(value) else {
        return;
    };
    if event.channel.is_empty() || event.ts.is_empty() {
        return;
    }
    send(
        link,
        Event::ReadReceipt {
            portal: portal(link, &event.channel),
            user: link.me_user.clone(),
            event: RemoteEventRef::new(ids::message_id(&link.team, &event.channel, &event.ts)),
        },
    );
}

fn on_self_left(link: &Link, value: Value) {
    let channel = value.get("channel").and_then(Value::as_str).unwrap_or_default();
    if channel.is_empty() {
        return;
    }
    send(
        link,
        Event::Member {
            portal: portal(link, channel),
            member: Member {
                user: link.me_user.clone(),
                membership: "leave".to_owned(),
            },
        },
    );
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct MemberEvent {
    channel: String,
    user: String,
    #[serde(rename = "type")]
    kind: String,
}

async fn on_member(link: &Link, value: Value) {
    let Ok(event) = serde_json::from_value::<MemberEvent>(value) else {
        return;
    };
    if event.channel.is_empty() || event.user.is_empty() {
        return;
    }
    let membership = if event.kind == "member_left_channel" {
        "leave"
    } else {
        "join"
    };
    let user = directory::resolve(&link.api, &link.directory, &link.team, &event.user).await;
    send(
        link,
        Event::Member {
            portal: portal(link, &event.channel),
            member: Member {
                user,
                membership: membership.to_owned(),
            },
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_real_messages_are_bridged() {
        assert!(is_message_subtype(""));
        assert!(is_message_subtype("thread_broadcast"));
        assert!(is_message_subtype("file_share"));
        assert!(!is_message_subtype("channel_join"));
        assert!(!is_message_subtype("message_replied"));
    }

    #[test]
    fn conversations_are_ordered_by_their_last_change() {
        let older = Channel {
            id: "C1".to_owned(),
            updated: 10,
            ..Channel::default()
        };
        let newer = Channel {
            id: "C2".to_owned(),
            updated: 20,
            ..Channel::default()
        };
        let mut list = [older, newer];
        list.sort_by_key(|channel| std::cmp::Reverse(recency(channel)));
        assert_eq!(list[0].id, "C2");
    }
}
