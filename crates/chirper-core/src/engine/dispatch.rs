//! The dispatcher: driver events in, database rows and wire events out.

use std::sync::Arc;

use chirper_bridge::model::{DeliveryStatus, SessionState};
use chirper_bridge::runtime::{Routed, RoutedEvent};
use tokio::sync::mpsc::Receiver;

use crate::ids::{self, SELF_USER_ID};
use crate::model::AccountState;
use crate::store::{Event, Member, Profile, Room, RoomFields};

use super::convert;
use super::{Inner, Result};

/// Drain routed driver events until the runtime is gone. Waits for the shell to catch up when it
/// has fallen far behind (see [`super::EVENT_BACKLOG`]).
pub(super) async fn run(inner: Arc<Inner>, mut rx: Receiver<RoutedEvent>) {
    while let Some(event) = rx.recv().await {
        if inner.closed.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        let shell = inner.events.clone();
        inner
            .backlog
            .wait_below(super::EVENT_BACKLOG, || shell.is_closed())
            .await;
        let network = event.network.clone();
        let login_id = event.login_id.clone();
        if let Err(err) = apply(&inner, &network, &login_id, event.routed) {
            let text = chirper_bridge::redact(&err.to_string());
            log::warn!("{network}/{login_id}: cannot apply event: {text}");
            inner.error(format!("{network}: {text}"));
        }
    }
    log::debug!("bridge event stream ended");
}

fn apply(inner: &Inner, network: &str, login_id: &str, routed: Routed) -> Result<()> {
    match routed {
        Routed::Chat { chat, meta, .. } => {
            let (room, parent) = convert::room_from_meta(network, login_id, &chat, &meta);
            if let Some((parent_chat, parent_portal)) = parent {
                ensure_room(inner, network, login_id, &parent_chat, &parent_portal, true)?;
                if !meta.parent_name.is_empty() {
                    inner.store.set_room_fields(
                        parent_chat.as_str(),
                        &RoomFields {
                            name: Some(meta.parent_name.clone()),
                            ..Default::default()
                        },
                    )?;
                }
            }
            let existed = inner.store.room(chat.as_str()).is_ok();
            inner.store.upsert_room(&room)?;
            inner.store.set_room_fields(
                chat.as_str(),
                &RoomFields {
                    name: Some(room.name.clone()),
                    topic: Some(room.topic.clone()),
                    avatar_url: Some(room.avatar_url.clone()),
                    portal_id: Some(room.portal_id.clone()),
                    remote_id: Some(room.remote_id.clone()),
                    parent_id: Some(room.parent_id.clone()),
                    parent_remote_id: Some(room.parent_remote_id.clone()),
                    login_id: Some(login_id.to_owned()),
                    ..Default::default()
                },
            )?;
            if let Some(user) = &meta.dm_user {
                remember_user(inner, network, user);
            }
            let _ = existed;
            emit_chat(inner, chat.as_str())?;
            Ok(())
        }
        Routed::ChatRemoved { chat, .. } => {
            let paths = inner.store.delete_room(chat.as_str())?;
            for path in paths {
                let _ = std::fs::remove_file(path);
            }
            inner.typing.lock().expect("typing").remove(chat.as_str());
            inner.emit(
                "chat_removed",
                serde_json::json!({ "roomId": chat.as_str() }),
            );
            Ok(())
        }
        Routed::Message {
            chat,
            portal,
            event,
            sender,
            message,
            edit_target,
            reply_to,
        } => message_event(
            inner,
            network,
            login_id,
            chat.as_str(),
            &portal,
            event.as_str(),
            sender.as_str(),
            &message,
            edit_target,
            reply_to,
        ),
        Routed::Redact { chat, target, .. } => {
            let redaction = inner.store.redact(target.as_str())?;
            if redaction.is_reaction {
                emit_message(inner, redaction.target_event_id.as_str())?;
                return Ok(());
            }
            inner.emit(
                "message_removed",
                serde_json::json!({
                    "roomId": chat.as_str(),
                    "eventId": target.as_str(),
                }),
            );
            emit_chat(inner, chat.as_str())?;
            Ok(())
        }
        Routed::Reactions {
            chat,
            target,
            reactions,
        } => {
            let summary = reactions
                .into_iter()
                .map(|reaction| {
                    let senders = reaction
                        .senders
                        .iter()
                        .map(|user| ids::sender_id(network, &user.id).into_inner())
                        .collect::<Vec<_>>();
                    (reaction.key, reaction.count, reaction.from_me, senders)
                })
                .collect::<Vec<_>>();
            inner
                .store
                .set_reaction_summary(chat.as_str(), target.as_str(), &summary)?;
            emit_message(inner, target.as_str())?;
            Ok(())
        }
        Routed::Typing {
            chat,
            senders,
            names,
            ..
        } => {
            let room_id = chat.as_str().to_owned();
            let payload = if senders.is_empty() {
                inner.typing.lock().expect("typing").remove(&room_id);
                serde_json::json!({ "roomId": room_id, "names": Vec::<String>::new() })
            } else {
                let names = names
                    .into_iter()
                    .filter(|n| !n.is_empty())
                    .collect::<Vec<_>>();
                inner
                    .typing
                    .lock()
                    .expect("typing")
                    .insert(room_id.clone(), names.clone());
                serde_json::json!({ "roomId": room_id, "names": names })
            };
            inner.emit("typing", payload);
            Ok(())
        }
        Routed::Member {
            chat,
            sender,
            user,
            membership,
        } => {
            let display = if user.display_name.is_empty() {
                user.username.clone()
            } else {
                user.display_name.clone()
            };
            inner.store.upsert_member(&Member {
                room_id: chat.as_str().to_owned(),
                user_id: sender.into_inner(),
                displayname: display,
                avatar_url: user.avatar_url.clone(),
                membership,
            })?;
            remember_user(inner, network, &user);
            emit_chat(inner, chat.as_str())?;
            Ok(())
        }
        Routed::ReadReceipt {
            chat,
            sender,
            target,
        } => {
            if sender.as_str() == SELF_USER_ID {
                let _ = inner.store.mark_read_up_to(chat.as_str(), target.as_str());
                emit_chat(inner, chat.as_str())?;
            }
            Ok(())
        }
        Routed::Delivery {
            chat,
            txn_id,
            status,
            error,
        } => {
            let found = inner.store.event_by_txn(chat.as_str(), &txn_id)?;
            let Some(event) = found else {
                return Ok(());
            };
            let status = match status {
                DeliveryStatus::Sent => "sent",
                DeliveryStatus::Delivered => "delivered",
                DeliveryStatus::Read => "read",
                DeliveryStatus::Failed => "failed",
            };
            inner
                .store
                .set_event_status(&event.event_id, status, &error)?;
            emit_message(inner, &event.event_id)?;
            Ok(())
        }
        Routed::State {
            state,
            error,
            message,
            profile,
        } => {
            let mut account = inner.account(network, login_id);
            account.state = account_state(state);
            account.error = error;
            account.message = message;
            if let Some((sender, user)) = profile {
                if !user.display_name.is_empty() {
                    account.name = user.display_name.clone();
                }
                if !user.avatar_url.is_empty() {
                    account.avatar_url = user.avatar_url.clone();
                }
                let _ = inner.store.put_login(&crate::store::Login {
                    network: network.to_owned(),
                    login_id: login_id.to_owned(),
                    name: account.name.clone(),
                    avatar_url: account.avatar_url.clone(),
                    remote_id: user.id.clone(),
                    ..Default::default()
                });
                let _ = inner.store.set_profile(&Profile {
                    user_id: sender.into_inner(),
                    displayname: user.display_name.clone(),
                    avatar_url: user.avatar_url.clone(),
                    extra_json: String::new(),
                });
                // The local user's own messages show the account name, not the sender id.
                let _ = inner.store.set_profile(&Profile {
                    user_id: SELF_USER_ID.to_owned(),
                    displayname: user.display_name,
                    avatar_url: user.avatar_url,
                    extra_json: String::new(),
                });
            }
            inner.set_account(account);
            if state.is_terminal() {
                inner.runtime.stop_login(network, login_id);
            }
            Ok(())
        }
        Routed::Error(message) => {
            inner.error(message);
            Ok(())
        }
    }
}

fn account_state(state: SessionState) -> AccountState {
    match state {
        SessionState::Connecting | SessionState::Backfilling => AccountState::Connecting,
        SessionState::Running => AccountState::Connected,
        SessionState::NeedsReauth | SessionState::BadCredentials => AccountState::BadCredentials,
        SessionState::LoggedOut => AccountState::LoggedOut,
        SessionState::TransientDisconnect => AccountState::TransientDisconnect,
        SessionState::UnknownError => AccountState::UnknownError,
    }
}

/// Store an incoming or echoed message and push it.
#[allow(clippy::too_many_arguments)]
fn message_event(
    inner: &Inner,
    network: &str,
    login_id: &str,
    room_id: &str,
    portal: &chirper_bridge::model::PortalRef,
    event_id: &str,
    sender: &str,
    message: &chirper_bridge::model::RemoteMessage,
    edit_target: Option<chirper_bridge::ids::EventId>,
    reply_to: Option<chirper_bridge::ids::EventId>,
) -> Result<()> {
    let room = ensure_room(
        inner,
        network,
        login_id,
        &chirper_bridge::ids::ChatId::new(room_id),
        portal,
        false,
    )?;
    let ts = if message.timestamp_ms > 0 {
        message.timestamp_ms
    } else {
        crate::store::now_ms()
    };

    if let Some(target) = edit_target {
        let stored = convert::content_json(
            &message.content,
            &convert::media_dir(&inner.data_dir),
            &inner.store,
        );
        inner
            .store
            .apply_edit(room_id, target.as_str(), &stored.json, ts)?;
        emit_message(inner, target.as_str())?;
        return Ok(());
    }

    // The network echoed a message this device sent: attach it to the optimistic row.
    if message.from_me
        && message.live
        && let Some(pending) = inner
            .store
            .pending_outgoing(room_id)?
            .into_iter()
            .find(|event| {
                crate::content::describe(&event.kind, &event.content_json, None).body
                    == crate::content::describe(
                        "message",
                        &convert::content_json(
                            &message.content,
                            &convert::media_dir(&inner.data_dir),
                            &inner.store,
                        )
                        .json,
                        None,
                    )
                    .body
            })
    {
        if !message.id.is_empty() {
            inner
                .store
                .set_event_remote(&pending.event_id, &message.id)?;
        }
        inner
            .store
            .set_event_status(&pending.event_id, "delivered", "")?;
        emit_message(inner, &pending.event_id)?;
        emit_chat(inner, room_id)?;
        return Ok(());
    }

    // The network confirmed a message this device sent: update it instead of duplicating it.
    if !message.id.is_empty()
        && let Some(existing) = inner.store.event_by_remote(room_id, &message.id)?
    {
        if message.from_me {
            inner
                .store
                .set_event_status(&existing.event_id, "delivered", "")?;
        }
        emit_message(inner, &existing.event_id)?;
        return Ok(());
    }

    let stored = convert::content_json(
        &message.content,
        &convert::media_dir(&inner.data_dir),
        &inner.store,
    );
    let mut event = Event {
        event_id: event_id.to_owned(),
        room_id: room.room_id.clone(),
        sender: sender.to_owned(),
        kind: "message".to_owned(),
        content_json: stored.json,
        ts,
        reply_to: reply_to.map(|id| id.into_inner()).unwrap_or_default(),
        status: "sent".to_owned(),
        remote_id: message.id.clone(),
        ..Default::default()
    };
    let inserted = inner.store.insert_event(&mut event)?;
    if !inserted {
        return Ok(());
    }
    if sender != SELF_USER_ID {
        let _ = inner.store.increment_unread(&event.event_id);
    }
    emit_message(inner, &event.event_id)?;
    if message.live {
        emit_chat(inner, room_id)?;
    }
    let _ = stored.kind;
    Ok(())
}

/// Make sure a room row exists, creating a minimal one when a driver reports a chat before its
/// metadata (or a message of a chat this device has not seen).
fn ensure_room(
    inner: &Inner,
    network: &str,
    login_id: &str,
    chat: &chirper_bridge::ids::ChatId,
    portal: &chirper_bridge::model::PortalRef,
    is_space: bool,
) -> Result<Room> {
    if let Ok(room) = inner.store.room(chat.as_str()) {
        return Ok(room);
    }
    let room = Room {
        room_id: chat.as_str().to_owned(),
        bridge_id: network.to_owned(),
        login_id: login_id.to_owned(),
        portal_id: convert::portal_key(portal),
        remote_id: portal.id.clone(),
        is_space,
        created_ts: crate::store::now_ms(),
        ..Default::default()
    };
    inner.store.upsert_room(&room)?;
    Ok(room)
}

fn remember_user(inner: &Inner, network: &str, user: &chirper_bridge::model::RemoteUser) {
    let user_id = ids::sender_id(network, &user.id).into_inner();
    let _ = inner.store.set_profile(&Profile {
        user_id,
        displayname: if user.display_name.is_empty() {
            user.username.clone()
        } else {
            user.display_name.clone()
        },
        avatar_url: user.avatar_url.clone(),
        extra_json: String::new(),
    });
}

fn emit_chat(inner: &Inner, room_id: &str) -> Result<()> {
    let mut chat = match inner.store.chat(room_id) {
        Ok(chat) => chat,
        Err(crate::store::Error::NotFound) => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    if let Some(names) = inner.typing.lock().expect("typing").get(room_id) {
        chat.typing = names.clone();
    }
    inner.emit("chat", &chat);
    Ok(())
}

fn emit_message(inner: &Inner, event_id: &str) -> Result<()> {
    let Ok(message) = inner.store.message_by_id(event_id) else {
        return Ok(());
    };
    inner.emit("message", &message);
    Ok(())
}
