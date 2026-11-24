//! Lightspeed rows → domain events, and the state those conversions need between responses.
//!
//! The reference's `pkg/connector/events.go` and `msgconv/from-meta.go` (AGPL-3.0, mautrix-meta) do
//! this with the Matrix bridge in the middle. Here the output is the runtime's [`Event`] and
//! [`RemoteMessage`] directly. Reactions are sent as the full set per message, because the model
//! replaces the stored set, so the conversion keeps each message's current reactions.

use std::collections::{BTreeMap, HashMap};

use chirper_bridge::model::{
    ChatMeta, Content, Reaction, RemoteEventRef, RemoteMessage, RemoteUser,
};
use chirper_bridge::Event;

use crate::ids;
use crate::ls::LsTable;
use crate::ls::table::{MessageRow, ThreadRow};

/// The thread types that are one-to-one conversations.
const ONE_TO_ONE_TYPES: [i64; 3] = [1, 7, 13];
/// How many recent messages per chat are remembered for read receipts.
const RECENT_MESSAGES: usize = 200;

/// What the conversions remember between responses.
#[derive(Debug, Default)]
pub struct Shared {
    /// The logged-in account id.
    pub me: i64,
    /// Users seen in contact rows, by id.
    pub users: HashMap<i64, RemoteUser>,
    /// The current reactions of each message, keyed by reaction id, per (chat, message).
    pub reactions: HashMap<(i64, String), BTreeMap<i64, Reaction>>,
    /// The most recent messages of each chat, as (timestamp, remote id), oldest first.
    pub recent: HashMap<i64, Vec<(i64, String)>>,
    /// The chat of every message seen, so an edit (which names no chat) can be routed.
    pub chats: HashMap<String, i64>,
    /// History requests in progress, by chat: the reference timestamp of the request and the
    /// messages collected for it. Messages newer than the reference are live, not history.
    pub history: HashMap<i64, (i64, Vec<RemoteMessage>)>,
    /// Whether the server said there is more history before the last collected page, by chat.
    pub history_more: HashMap<i64, bool>,
}

impl Shared {
    pub fn new(me: i64) -> Self {
        Self {
            me,
            ..Self::default()
        }
    }

    fn user(&self, id: i64) -> RemoteUser {
        self.users.get(&id).cloned().unwrap_or_else(|| RemoteUser {
            id: ids::user(id),
            ..RemoteUser::default()
        })
    }

    fn remember(&mut self, chat: i64, timestamp_ms: i64, remote_id: &str) {
        let recent = self.recent.entry(chat).or_default();
        recent.push((timestamp_ms, remote_id.to_owned()));
        if recent.len() > RECENT_MESSAGES {
            let excess = recent.len() - RECENT_MESSAGES;
            recent.drain(..excess);
        }
    }
}

/// Every event one lightspeed table implies, in the order the server stated the changes.
///
/// `live` is true for messages that arrived in real time; a message that answers an open history
/// request is collected instead and is never a live event.
pub fn events(table: &LsTable, shared: &mut Shared, live: bool) -> Vec<Event> {
    let mut out = Vec::new();
    for contact in &table.contacts {
        shared.users.insert(
            contact.id,
            RemoteUser {
                id: ids::user(contact.id),
                display_name: contact.name.clone(),
                avatar_url: contact.profile_picture_url.clone(),
                username: contact.username.clone(),
                phone: String::new(),
            },
        );
    }
    for thread in &table.threads {
        if thread.thread_key != 0 {
            out.push(Event::Chat(chat_meta(thread)));
        }
    }
    for thread in &table.deleted_threads {
        out.push(Event::ChatRemoved(ids::portal(*thread)));
    }
    for row in &table.messages {
        message_event(row, shared, live, &mut out);
    }
    for row in &table.message_ranges {
        shared.history_more.insert(row.thread_key, row.has_more_before);
    }
    for edit in &table.edits {
        let remote_id = ids::message(&edit.message_id);
        let Some(&chat) = shared.chats.get(&remote_id) else {
            continue;
        };
        let content = if edit.text.is_empty() {
            Content::Unsupported {
                body: "Edited message".to_owned(),
            }
        } else {
            Content::Text {
                body: edit.text.clone(),
                formatted: None,
            }
        };
        out.push(Event::Message(RemoteMessage {
            portal: ids::portal(chat),
            id: String::new(),
            part: String::new(),
            sender: RemoteUser::default(),
            timestamp_ms: 0,
            content,
            reply_to: None,
            edit_of: Some(remote_id),
            live,
            from_me: false,
        }));
    }
    for (thread, message_id) in &table.deleted_messages {
        out.push(Event::Redact {
            portal: ids::portal(*thread),
            event: RemoteEventRef::new(ids::message(message_id)),
        });
    }
    for row in &table.reactions_v2 {
        reaction_event(row, shared, &mut out);
    }
    for row in &table.read_receipts {
        read_receipt_event(row.thread_key, row.contact_id, row.watermark_ms, shared, &mut out);
    }
    for row in &table.typing {
        out.push(Event::Typing {
            portal: ids::portal(row.thread_key),
            senders: if row.is_typing {
                vec![shared.user(row.sender_id)]
            } else {
                Vec::new()
            },
        });
    }
    out
}

fn chat_meta(thread: &ThreadRow) -> ChatMeta {
    ChatMeta {
        portal: ids::portal(thread.thread_key),
        name: thread.thread_name.clone(),
        topic: String::new(),
        avatar_url: thread.thread_picture_url.clone(),
        is_dm: ONE_TO_ONE_TYPES.contains(&thread.thread_type),
        is_space: false,
        member_count: 0,
        parent: None,
        parent_name: String::new(),
        dm_user: None,
    }
}

fn message_event(row: &MessageRow, shared: &mut Shared, live: bool, out: &mut Vec<Event>) {
    if row.message_id.is_empty() {
        return;
    }
    let remote_id = ids::message(&row.message_id);
    if row.is_unsent {
        out.push(Event::Redact {
            portal: ids::portal(row.thread_key),
            event: RemoteEventRef::new(remote_id),
        });
        return;
    }
    let history = shared
        .history
        .get(&row.thread_key)
        .is_some_and(|(reference_ms, _)| row.timestamp_ms <= *reference_ms);
    let message = RemoteMessage {
        portal: ids::portal(row.thread_key),
        id: remote_id.clone(),
        part: String::new(),
        sender: shared.user(row.sender_id),
        timestamp_ms: row.timestamp_ms,
        content: content_of(row),
        reply_to: (!row.reply_source_id.is_empty()).then(|| ids::message(&row.reply_source_id)),
        edit_of: None,
        live: live && !history,
        from_me: row.sender_id == shared.me,
    };
    shared.remember(row.thread_key, row.timestamp_ms, &remote_id);
    shared.chats.insert(remote_id.clone(), row.thread_key);
    match shared.history.get_mut(&row.thread_key) {
        Some((reference_ms, collected)) if row.timestamp_ms <= *reference_ms => {
            collected.push(message);
        }
        _ => out.push(Event::Message(message)),
    }
}

/// The content of a message row. Attachments are not decoded yet, so a message without text is
/// shown as unsupported rather than dropped.
fn content_of(row: &MessageRow) -> Content {
    if row.is_admin_message {
        return Content::Notice {
            body: row.text.clone(),
        };
    }
    if !row.text.is_empty() {
        return Content::Text {
            body: row.text.clone(),
            formatted: None,
        };
    }
    let body = if row.sticker_id != 0 {
        "Sticker"
    } else {
        "Attachment"
    };
    Content::Unsupported {
        body: body.to_owned(),
    }
}

fn reaction_event(row: &crate::ls::table::ReactionV2Row, shared: &mut Shared, out: &mut Vec<Event>) {
    let key = (row.thread_key, row.message_id.clone());
    let reactions = shared.reactions.entry(key).or_default();
    if row.count <= 0 {
        reactions.remove(&row.reaction_fbid);
    } else {
        reactions.insert(
            row.reaction_fbid,
            Reaction {
                key: row.reaction_literal.clone(),
                count: row.count,
                from_me: row.viewer_is_reactor,
                senders: Vec::new(),
            },
        );
    }
    let full: Vec<Reaction> = reactions.values().cloned().collect();
    out.push(Event::Reactions {
        portal: ids::portal(row.thread_key),
        target: RemoteEventRef::new(ids::message(&row.message_id)),
        reactions: full,
    });
}

fn read_receipt_event(
    thread: i64,
    contact: i64,
    watermark_ms: i64,
    shared: &Shared,
    out: &mut Vec<Event>,
) {
    let Some(target) = shared
        .recent
        .get(&thread)
        .and_then(|recent| recent.iter().rev().find(|(ts, _)| *ts <= watermark_ms))
        .map(|(_, id)| id.clone())
    else {
        return;
    };
    out.push(Event::ReadReceipt {
        portal: ids::portal(thread),
        user: shared.user(contact),
        event: RemoteEventRef::new(target),
    });
}
