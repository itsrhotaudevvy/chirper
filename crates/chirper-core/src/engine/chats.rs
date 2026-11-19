//! Chat and message commands: the user's own actions.

use std::path::Path;
use std::sync::Arc;

use chirper_bridge::model::{MediaInfo, PortalRef};
use chirper_bridge::network::Command;

use super::convert;
use super::{Core, Error, Inner, Result, validate};
use crate::content;
use crate::ids::{self, SELF_USER_ID};
use crate::model::{Chat, ChatFlags, Message};
use crate::store::{Event, Profile, Room, RoomFields};

/// A local file to send as an attachment.
#[derive(Debug, Clone, Copy)]
pub struct MediaUpload<'a> {
    pub path: &'a Path,
    pub mime_type: &'a str,
    pub file_name: &'a str,
    pub caption: &'a str,
    /// `image`, `video`, `audio`, `voice`, `file` or `sticker`.
    pub kind: &'a str,
    pub reply_to: Option<&'a str>,
    pub txn_id: &'a str,
}

/// Where a room lives: the driver's portal and the login to talk to it through.
struct RoomRef {
    room: Room,
    portal: PortalRef,
    network: String,
    login_id: String,
}

impl Inner {
    fn room_ref(&self, room_id: &str) -> Result<RoomRef> {
        validate::room_id(room_id)?;
        let room = match self.store.room(room_id) {
            Ok(room) => room,
            Err(crate::store::Error::NotFound) => return Err(Error::NotFound),
            Err(err) => return Err(err.into()),
        };
        if room.deleted {
            return Err(Error::NotFound);
        }
        if room.login_id.is_empty() || room.bridge_id.is_empty() {
            return Err(Error::Unsupported("sending to this chat".to_owned()));
        }
        Ok(RoomRef {
            portal: convert::portal_of(&room),
            network: room.bridge_id.clone(),
            login_id: room.login_id.clone(),
            room,
        })
    }

    fn emit_chat(&self, room_id: &str) {
        if let Ok(mut chat) = self.store.chat(room_id) {
            if let Some(names) = self.typing.lock().expect("typing").get(room_id) {
                chat.typing = names.clone();
            }
            self.emit("chat", &chat);
        }
    }

    fn emit_message(&self, event_id: &str) {
        if let Ok(message) = self.store.message_by_id(event_id) {
            self.emit("message", &message);
        }
    }

    /// Store an outgoing event and push it before the network has answered.
    fn optimistic(
        &self,
        room: &Room,
        content_json: String,
        reply_to: &str,
        txn_id: &str,
    ) -> Result<Event> {
        let mut event = Event {
            event_id: ids::local_event_id().into_inner(),
            room_id: room.room_id.clone(),
            sender: SELF_USER_ID.to_owned(),
            kind: "message".to_owned(),
            content_json,
            ts: crate::store::now_ms(),
            txn_id: txn_id.to_owned(),
            reply_to: reply_to.to_owned(),
            status: "sending".to_owned(),
            ..Default::default()
        };
        self.store.insert_event(&mut event)?;
        self.emit_message(&event.event_id);
        Ok(event)
    }

    /// Record the network's verdict on an outgoing event and push it again.
    fn finish_send(&self, room_id: &str, event_id: &str, status: &str, error: &str) {
        let _ = self.store.set_event_status(event_id, status, error);
        self.emit_message(event_id);
        self.emit_chat(room_id);
    }

    /// The remote id of a local event, for replies and edits.
    fn remote_of(&self, room_id: &str, event_id: &str) -> Result<String> {
        let event = self.store.event(event_id)?;
        if event.room_id != room_id {
            return Err(Error::InvalidInput(
                "event does not belong to this chat".to_owned(),
            ));
        }
        Ok(event.remote_id)
    }
}

impl Core {
    /// `sendText`: stores the message, returns its local event id, and sends it in the background.
    pub async fn send_text(
        &self,
        room_id: &str,
        text: &str,
        reply_to: Option<&str>,
        txn_id: &str,
    ) -> Result<String> {
        self.inner().require_open()?;
        validate::room_id(room_id)?;
        validate::text(text)?;
        validate::token(txn_id)?;
        let reply_to = match reply_to {
            Some(id) => {
                validate::event_id(id)?;
                id
            }
            None => "",
        };
        let inner = self.inner.clone();
        let room_ref = inner.room_ref(room_id)?;
        let reply_remote = if reply_to.is_empty() {
            None
        } else {
            Some(inner.remote_of(room_id, reply_to)?)
        };
        let event = inner.optimistic(
            &room_ref.room,
            content::text_content(text, None),
            reply_to,
            txn_id,
        )?;
        let event_id = event.event_id.clone();
        let core = self.clone();
        let command = Command::SendText {
            portal: room_ref.portal,
            text: text.to_owned(),
            reply_to: reply_remote.filter(|id| !id.is_empty()),
            txn_id: txn_id.to_owned(),
        };
        let (network, login_id) = (room_ref.network, room_ref.login_id);
        let room = room_id.to_owned();
        tokio::spawn(async move {
            core.deliver(room, event_id, network, login_id, command)
                .await;
        });
        Ok(event.event_id)
    }

    /// `sendMedia`: copies the file into the media store, then sends it.
    pub async fn send_media(&self, room_id: &str, upload: MediaUpload<'_>) -> Result<String> {
        self.inner().require_open()?;
        validate::room_id(room_id)?;
        let path = validate::existing_file(upload.path)?;
        validate::mime_type(upload.mime_type)?;
        validate::file_name(upload.file_name)?;
        validate::caption(upload.caption)?;
        validate::media_kind(upload.kind)?;
        validate::token(upload.txn_id)?;
        let reply_to = match upload.reply_to {
            Some(id) => {
                validate::event_id(id)?;
                id
            }
            None => "",
        };
        let inner = self.inner.clone();
        let room_ref = inner.room_ref(room_id)?;
        let reply_remote = if reply_to.is_empty() {
            None
        } else {
            Some(inner.remote_of(room_id, reply_to)?)
        };
        // One copy of the file for the whole send: the store and the driver share it.
        let data = Arc::new(std::fs::read(&path)?);
        let info = MediaInfo {
            mime: upload.mime_type.to_owned(),
            file_name: upload.file_name.to_owned(),
            size: data.len(),
            ..Default::default()
        };
        let media = convert::save_media(
            &convert::media_dir(&inner.data_dir),
            &inner.store,
            &info,
            &data,
            None,
        )?
        .ok_or_else(|| Error::InvalidInput("file is empty".to_owned()))?;
        let body = content::media_content(&media_kind(upload.kind), upload.caption, &media);
        let event = inner.optimistic(&room_ref.room, body, reply_to, upload.txn_id)?;
        let event_id = event.event_id.clone();
        let command = Command::SendMedia {
            portal: room_ref.portal,
            data: Arc::clone(&data),
            mime: upload.mime_type.to_owned(),
            file_name: upload.file_name.to_owned(),
            caption: upload.caption.to_owned(),
            kind: upload.kind.to_owned(),
            reply_to: reply_remote.filter(|id| !id.is_empty()),
            txn_id: upload.txn_id.to_owned(),
        };
        let (network, login_id) = (room_ref.network, room_ref.login_id);
        let room = room_id.to_owned();
        let core = self.clone();
        let spawned_id = event_id.clone();
        tokio::spawn(async move {
            core.deliver(room, spawned_id, network, login_id, command)
                .await;
        });
        Ok(event_id)
    }

    async fn deliver(
        self,
        room_id: String,
        event_id: String,
        network: String,
        login_id: String,
        command: Command,
    ) {
        let inner = self.inner.clone();
        match inner.runtime.command(&network, &login_id, command).await {
            Ok(ack) => {
                if let Some(remote) = ack.remote_event_id {
                    let _ = inner.store.set_event_remote(&event_id, &remote);
                }
                inner.finish_send(&room_id, &event_id, "sent", "");
            }
            Err(err) => {
                let text = chirper_bridge::redact(&err.to_string());
                log::warn!("{network}/{login_id}: send failed: {text}");
                inner.finish_send(&room_id, &event_id, "failed", &text);
            }
        }
    }

    /// `edit`: stores the new text on the target event and tells the network.
    pub async fn edit(&self, room_id: &str, event_id: &str, text: &str) -> Result<()> {
        self.inner().require_open()?;
        validate::room_id(room_id)?;
        validate::event_id(event_id)?;
        validate::text(text)?;
        let inner = self.inner.clone();
        let room_ref = inner.room_ref(room_id)?;
        let target = inner.remote_of(room_id, event_id)?;
        if target.is_empty() {
            return Err(Error::Failed(
                "this message has not reached the network yet".to_owned(),
            ));
        }
        inner.store.apply_edit(
            room_id,
            event_id,
            &content::text_content(text, None),
            crate::store::now_ms(),
        )?;
        inner.emit_message(event_id);
        inner.emit_chat(room_id);
        let command = Command::Edit {
            portal: room_ref.portal,
            target,
            text: text.to_owned(),
        };
        self.run_command(room_ref.network, room_ref.login_id, command)
            .await
    }

    /// `redact`: removes a message.
    pub async fn redact(&self, room_id: &str, event_id: &str) -> Result<()> {
        self.inner().require_open()?;
        validate::room_id(room_id)?;
        validate::event_id(event_id)?;
        let inner = self.inner.clone();
        let room_ref = inner.room_ref(room_id)?;
        let target = inner.remote_of(room_id, event_id)?;
        let redaction = inner.store.redact(event_id)?;
        if redaction.is_reaction {
            inner.emit_message(&redaction.target_event_id);
            return Ok(());
        }
        inner.emit(
            "message_removed",
            serde_json::json!({
                "roomId": room_id,
                "eventId": event_id,
            }),
        );
        inner.emit_chat(room_id);
        if target.is_empty() {
            return Ok(());
        }
        let command = Command::Redact {
            portal: room_ref.portal,
            target,
        };
        self.run_command(room_ref.network, room_ref.login_id, command)
            .await
    }

    /// `react`: adds a reaction.
    pub async fn react(&self, room_id: &str, event_id: &str, key: &str) -> Result<()> {
        self.reaction(room_id, event_id, key, true).await
    }

    /// `unreact`: removes the user's reaction.
    pub async fn unreact(&self, room_id: &str, event_id: &str, key: &str) -> Result<()> {
        self.reaction(room_id, event_id, key, false).await
    }

    async fn reaction(&self, room_id: &str, event_id: &str, key: &str, add: bool) -> Result<()> {
        self.inner().require_open()?;
        validate::room_id(room_id)?;
        validate::event_id(event_id)?;
        validate::reaction_key(key)?;
        let inner = self.inner.clone();
        let room_ref = inner.room_ref(room_id)?;
        let target = inner.remote_of(room_id, event_id)?;
        if add {
            inner.store.add_reaction(&crate::store::Reaction {
                event_id: ids::reaction_event_id(
                    &room_ref.network,
                    room_id,
                    event_id,
                    SELF_USER_ID,
                    key,
                )
                .into_inner(),
                room_id: room_id.to_owned(),
                target_event_id: event_id.to_owned(),
                sender: SELF_USER_ID.to_owned(),
                key: key.to_owned(),
                ts: crate::store::now_ms(),
            })?;
        } else {
            let existing = inner.store.find_reactions(event_id, SELF_USER_ID)?;
            if let Some(found) = existing.iter().find(|r| r.key == key) {
                inner.store.remove_reaction_by_event_id(&found.event_id)?;
            }
        }
        inner.emit_message(event_id);
        if target.is_empty() {
            return Ok(());
        }
        let command = if add {
            Command::React {
                portal: room_ref.portal,
                target,
                key: key.to_owned(),
            }
        } else {
            Command::Unreact {
                portal: room_ref.portal,
                target,
                key: key.to_owned(),
            }
        };
        self.run_command(room_ref.network, room_ref.login_id, command)
            .await
    }

    /// `markRead`: moves the read marker and reports it to the network.
    pub async fn mark_read(&self, room_id: &str, event_id: &str) -> Result<()> {
        self.inner().require_open()?;
        validate::room_id(room_id)?;
        validate::event_id(event_id)?;
        let inner = self.inner.clone();
        let room_ref = inner.room_ref(room_id)?;
        inner.store.mark_read_up_to(room_id, event_id)?;
        inner.emit_chat(room_id);
        let remote = inner.store.event(event_id)?.remote_id;
        let command = Command::Read {
            portal: room_ref.portal,
            event: (!remote.is_empty()).then_some(remote),
        };
        // A read receipt is best effort: a network without them must not fail the local change.
        let _ = inner
            .runtime
            .command(&room_ref.network, &room_ref.login_id, command)
            .await;
        Ok(())
    }

    /// `markUnread`.
    pub async fn mark_unread(&self, room_id: &str, unread: bool) -> Result<()> {
        self.inner().require_open()?;
        validate::room_id(room_id)?;
        self.inner().store.mark_unread(room_id, unread)?;
        self.inner().emit_chat(room_id);
        Ok(())
    }

    /// `setTyping`: tells the network the user is typing.
    pub async fn set_typing(&self, room_id: &str, typing: bool) -> Result<()> {
        self.inner().require_open()?;
        validate::room_id(room_id)?;
        let inner = self.inner.clone();
        let room_ref = inner.room_ref(room_id)?;
        let command = Command::Typing {
            portal: room_ref.portal,
            typing,
        };
        let _ = inner
            .runtime
            .command(&room_ref.network, &room_ref.login_id, command)
            .await;
        Ok(())
    }

    /// `setChatFlags`: local flags only; nothing is sent.
    pub async fn set_chat_flags(&self, room_id: &str, flags: &ChatFlags) -> Result<()> {
        self.inner().require_open()?;
        validate::room_id(room_id)?;
        self.inner().store.set_room_fields(
            room_id,
            &RoomFields {
                pinned: flags.pinned,
                archived: flags.archived,
                muted_until: flags.muted_until,
                ..Default::default()
            },
        )?;
        self.inner().emit_chat(room_id);
        Ok(())
    }

    /// `getChat` for a chat the UI already knows.
    pub async fn chat(&self, room_id: &str) -> Result<Chat> {
        self.inner().require_open()?;
        validate::room_id(room_id)?;
        Ok(self.inner().store.chat(room_id)?)
    }

    /// `listMessages` for a chat, with the same validation as the wire method.
    pub async fn messages(&self, room_id: &str, before: i64, limit: u32) -> Result<Vec<Message>> {
        self.list_messages(room_id, before, limit).await
    }

    /// `loadOlder`: fetch one page of history from the network. `false` at the beginning.
    pub async fn load_older(&self, room_id: &str) -> Result<bool> {
        self.inner().require_open()?;
        validate::room_id(room_id)?;
        let inner = self.inner.clone();
        let room_ref = inner.room_ref(room_id)?;
        let (cursor, done) = inner.store.backfill(room_id)?;
        if done {
            return Ok(false);
        }
        let page = inner
            .runtime
            .backfill(
                &room_ref.network,
                &room_ref.login_id,
                room_ref.portal.clone(),
                (!cursor.is_empty()).then_some(cursor),
                chirper_bridge::model::DEFAULT_BACKFILL_PAGE,
            )
            .await?;
        let mut events = Vec::with_capacity(page.messages.len());
        let mut profiles = Vec::with_capacity(page.messages.len());
        for message in &page.messages {
            if let Some((event, profile)) = history_row(&inner, &room_ref, message) {
                events.push(event);
                profiles.push(profile);
            }
        }
        // The page, its profiles and its cursor commit together: one write per page.
        inner.store.store_backfill_page(
            room_id,
            &mut events,
            &profiles,
            page.cursor.as_deref().unwrap_or(""),
            !page.has_more,
        )?;
        if !page.messages.is_empty() {
            inner.emit_chat(room_id);
        }
        Ok(page.has_more)
    }

    /// `syncHistory`: pull history for one network, or for every network when empty.
    pub async fn sync_history(&self, network: &str) -> Result<()> {
        self.inner().require_open()?;
        if !network.is_empty() {
            validate::network_id(network)?;
        }
        let rooms = self.inner().store.list_chats(&crate::model::ChatFilter {
            network: network.to_owned(),
            ..Default::default()
        })?;
        for chat in rooms {
            for _ in 0..20 {
                match self.load_older(&chat.room_id).await {
                    Ok(true) => continue,
                    Ok(false) => break,
                    Err(err) => {
                        log::debug!(
                            "history of {}: {}",
                            chat.room_id,
                            chirper_bridge::redact(&err.to_string())
                        );
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    async fn run_command(&self, network: String, login_id: String, command: Command) -> Result<()> {
        self.inner()
            .runtime
            .command(&network, &login_id, command)
            .await?;
        Ok(())
    }
}

fn media_kind(kind: &str) -> String {
    match kind {
        "image" | "video" | "audio" | "voice" | "sticker" => kind.to_owned(),
        _ => "file".to_owned(),
    }
}

/// The row of one backfilled message, or `None` when it has no id or is already stored. History
/// never becomes unread and never moves the chat; the caller writes the row in one transaction.
fn history_row(
    inner: &Inner,
    room: &RoomRef,
    message: &chirper_bridge::model::RemoteMessage,
) -> Option<(Event, Profile)> {
    if message.id.is_empty() {
        return None;
    }
    if inner
        .store
        .event_by_remote(&room.room.room_id, &message.id)
        .ok()
        .flatten()
        .is_some()
    {
        return None;
    }
    let sender = ids::sender_id(&room.network, &message.sender.id);
    let profile = Profile {
        user_id: sender.as_str().to_owned(),
        displayname: message.sender.display_name.clone(),
        avatar_url: message.sender.avatar_url.clone(),
        extra_json: String::new(),
    };
    let stored = convert::content_json(
        &message.content,
        &convert::media_dir(&inner.data_dir),
        &inner.store,
    );
    let event = Event {
        event_id: ids::event_id_for(
            &room.network,
            &room.room.room_id,
            &message.id,
            &message.part,
        )
        .into_inner(),
        room_id: room.room.room_id.clone(),
        sender: sender.into_inner(),
        kind: "message".to_owned(),
        content_json: stored.json,
        ts: if message.timestamp_ms > 0 {
            message.timestamp_ms
        } else {
            crate::store::now_ms()
        },
        status: "sent".to_owned(),
        remote_id: message.id.clone(),
        ..Default::default()
    };
    Some((event, profile))
}
