//! The Discord gateway: one WebSocket connection that turns dispatch events into bridge events.
//!
//! Reconnection is the runtime's job. This module opens one connection, identifies, keeps the
//! heartbeat, forwards events and ends when the socket closes.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use log::{debug, warn};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use chirper_bridge::model::{Event, Member, PortalRef, RemoteEventRef, SessionState};

use crate::api::Api;
use crate::map;

/// The gateway intents: guilds, guild and direct messages, reactions and message content.
const INTENTS: u64 = 1 | 512 | 4096 | 8192 | 16384 | 32768;

/// Run one gateway connection. Returns when the socket ends.
pub async fn run(api: Api, gateway_url: String, token: String, tx: UnboundedSender<Event>) {
    let url = format!("{gateway_url}/?v=10&encoding=json");
    let request = match url.into_client_request() {
        Ok(request) => request,
        Err(err) => {
            let _ = tx.send(Event::State {
                state: SessionState::TransientDisconnect,
                error: format!("gateway request: {err}"),
                message: String::new(),
                profile: None,
            });
            return;
        }
    };
    let (mut socket, _response) = match tokio_tungstenite::connect_async(request).await {
        Ok(pair) => pair,
        Err(err) => {
            let _ = tx.send(Event::State {
                state: SessionState::TransientDisconnect,
                error: format!("gateway connect: {err}"),
                message: String::new(),
                profile: None,
            });
            return;
        }
    };

    let mut sequence: Option<i64> = None;
    let mut heartbeat: Option<tokio::time::Interval> = None;
    let mut identified = false;

    loop {
        let tick = async {
            match heartbeat.as_mut() {
                Some(interval) => {
                    interval.tick().await;
                }
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            message = socket.next() => {
                let Some(message) = message else { break };
                let message = match message {
                    Ok(message) => message,
                    Err(err) => {
                        debug!("gateway socket error: {err}");
                        break;
                    }
                };
                let text = match message {
                    WsMessage::Text(text) => text,
                    WsMessage::Close(_) => break,
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
                if let Some(seq) = payload.get("s").and_then(|s| s.as_i64()) {
                    sequence = Some(seq);
                }
                let op = payload.get("op").and_then(|o| o.as_i64()).unwrap_or(-1);
                match op {
                    10 => {
                        // Hello: start heartbeating at the requested interval.
                        let interval = payload
                            .pointer("/d/heartbeat_interval")
                            .and_then(|i| i.as_u64())
                            .unwrap_or(41_250);
                        let mut timer = tokio::time::interval(Duration::from_millis(interval));
                        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                        timer.tick().await;
                        heartbeat = Some(timer);
                        if !identified {
                            let identify = json!({
                                "op": 2,
                                "d": {
                                    "token": token,
                                    "intents": INTENTS,
                                    "properties": {
                                        "os": std::env::consts::OS,
                                        "browser": "Chirper",
                                        "device": "Chirper"
                                    },
                                    "compress": false
                                }
                            });
                            if socket.send(WsMessage::Text(identify.to_string().into())).await.is_err() {
                                break;
                            }
                            identified = true;
                        }
                    }
                    11 => {}
                    1 => {
                        send_heartbeat(&mut socket, sequence).await;
                    }
                    7 => {
                        debug!("gateway asked to reconnect");
                        break;
                    }
                    9 => {
                        debug!("gateway invalidated the session");
                        break;
                    }
                    0 => {
                        let kind = payload.get("t").and_then(|t| t.as_str()).unwrap_or_default();
                        let data = payload.get("d").cloned().unwrap_or(Value::Null);
                        dispatch(kind, data, &api, &tx).await;
                    }
                    _ => {}
                }
            }
            () = tick => {
                send_heartbeat(&mut socket, sequence).await;
            }
        }
    }

    let _ = socket.close(None).await;
    if tx
        .send(Event::State {
            state: SessionState::TransientDisconnect,
            error: String::new(),
            message: String::new(),
            profile: None,
        })
        .is_err()
    {
        return;
    }
    drop(tx);
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

/// Handle one dispatch event.
async fn dispatch(kind: &str, data: Value, api: &Api, tx: &UnboundedSender<Event>) {
    match kind {
        "READY" => ready(&data, api, tx),
        "GUILD_CREATE" => {
            if let Ok(guild) = serde_json::from_value::<map::Guild>(data.clone()) {
                if !guild.unavailable {
                    let _ = tx.send(Event::Chat(map::guild_meta(&guild)));
                    spawn_guild_channels(
                        api.clone(),
                        tx.clone(),
                        guild.id.clone(),
                        guild.name.clone(),
                    );
                }
            }
        }
        "MESSAGE_CREATE" | "MESSAGE_UPDATE" => {
            if let Ok(message) = serde_json::from_value::<map::Message>(data) {
                if map::is_system(&message)
                    || (message.author.bot
                        && message.content.is_empty()
                        && message.attachments.is_empty())
                {
                    return;
                }
                let api = api.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    let live = true;
                    let remote = map::to_remote_message(&api, &message, live).await;
                    let _ = tx.send(Event::Message(remote));
                });
            }
        }
        "MESSAGE_DELETE" => {
            let channel = data
                .get("channel_id")
                .and_then(|c| c.as_str())
                .unwrap_or_default();
            let id = data.get("id").and_then(|i| i.as_str()).unwrap_or_default();
            if !channel.is_empty() && !id.is_empty() {
                let _ = tx.send(Event::Redact {
                    portal: portal(channel),
                    event: RemoteEventRef::new(id),
                });
            }
        }
        "MESSAGE_REACTION_ADD" | "MESSAGE_REACTION_REMOVE" => {
            let channel = data
                .get("channel_id")
                .and_then(|c| c.as_str())
                .unwrap_or_default();
            let id = data
                .get("message_id")
                .and_then(|m| m.as_str())
                .unwrap_or_default();
            if channel.is_empty() || id.is_empty() {
                return;
            }
            let api = api.clone();
            let tx = tx.clone();
            let portal = portal(channel);
            let message_id = id.to_owned();
            tokio::spawn(async move {
                // Discord reports one reactor per event; the UI wants the summary, so read it.
                if let Ok(message) = api.message(&portal.id, &message_id).await {
                    let _ = tx.send(Event::Reactions {
                        portal,
                        target: RemoteEventRef::new(message_id),
                        reactions: map::reactions(&message),
                    });
                }
            });
        }
        "TYPING_START" => {
            let channel = data
                .get("channel_id")
                .and_then(|c| c.as_str())
                .unwrap_or_default();
            if channel.is_empty() {
                return;
            }
            let user = data
                .pointer("/member/user")
                .cloned()
                .and_then(|value| serde_json::from_value::<map::User>(value).ok())
                .or_else(|| {
                    data.get("user_id")
                        .and_then(|u| u.as_str())
                        .map(|id| map::User {
                            id: id.to_owned(),
                            ..Default::default()
                        })
                });
            if let Some(user) = user {
                let _ = tx.send(Event::Typing {
                    portal: portal(channel),
                    senders: vec![map::to_remote_user(&user)],
                });
            }
        }
        "CHANNEL_CREATE" | "CHANNEL_UPDATE" => {
            if let Ok(channel) = serde_json::from_value::<map::Channel>(data) {
                if let Some(meta) = map::chat_meta(&channel, "") {
                    let _ = tx.send(Event::Chat(meta));
                }
            }
        }
        "CHANNEL_DELETE" => {
            if let Ok(channel) = serde_json::from_value::<map::Channel>(data) {
                let _ = tx.send(Event::ChatRemoved(portal(channel.id)));
            }
        }
        "GUILD_MEMBER_ADD" | "GUILD_MEMBER_REMOVE" | "GUILD_MEMBER_UPDATE" => {
            let guild = data
                .get("guild_id")
                .and_then(|g| g.as_str())
                .unwrap_or_default();
            if guild.is_empty() {
                return;
            }
            let Some(user) = data
                .get("user")
                .cloned()
                .and_then(|value| serde_json::from_value::<map::User>(value).ok())
            else {
                return;
            };
            let membership = match kind {
                "GUILD_MEMBER_ADD" => "join",
                "GUILD_MEMBER_REMOVE" => "leave",
                _ => "join",
            };
            let _ = tx.send(Event::Member {
                portal: portal(guild),
                member: Member {
                    user: map::to_remote_user(&user),
                    membership: membership.to_owned(),
                },
            });
        }
        _ => {}
    }
}

fn ready(data: &Value, api: &Api, tx: &UnboundedSender<Event>) {
    let profile = data
        .get("user")
        .cloned()
        .and_then(|value| serde_json::from_value::<map::User>(value).ok())
        .map(|user| map::to_remote_user(&user));
    let _ = tx.send(Event::State {
        state: SessionState::Running,
        error: String::new(),
        message: String::new(),
        profile,
    });

    if let Some(guilds) = data.get("guilds").and_then(|g| g.as_array()) {
        for guild in guilds {
            let Ok(guild) = serde_json::from_value::<map::Guild>(guild.clone()) else {
                continue;
            };
            if guild.unavailable {
                continue;
            }
            let _ = tx.send(Event::Chat(map::guild_meta(&guild)));
            spawn_guild_channels(
                api.clone(),
                tx.clone(),
                guild.id.clone(),
                guild.name.clone(),
            );
        }
    }
    if let Some(channels) = data.get("private_channels").and_then(|c| c.as_array()) {
        for channel in channels {
            if let Ok(channel) = serde_json::from_value::<map::Channel>(channel.clone()) {
                if let Some(meta) = map::chat_meta(&channel, "") {
                    let _ = tx.send(Event::Chat(meta));
                }
            }
        }
    }
}

fn spawn_guild_channels(
    api: Api,
    tx: UnboundedSender<Event>,
    guild_id: String,
    guild_name: String,
) {
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

fn portal(channel: impl Into<String>) -> PortalRef {
    PortalRef {
        id: channel.into(),
        receiver: String::new(),
    }
}
