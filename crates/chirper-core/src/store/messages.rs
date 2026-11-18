//! Rendering stored events as the wire `Message`, plus listing and search.

use rusqlite::params;

use super::events::{Event, Reaction};
use super::{Result, Store, like_pattern};
use crate::content::{self, Described};
use crate::model::{ChatPreview, Message, MessageStatus, Reaction as MessageReaction, ReplyTo};

/// The sender of the local user; messages from it are "mine" and never unread.
use crate::ids::SELF_USER_ID;

fn status_of(event: &Event) -> MessageStatus {
    match event.status.as_str() {
        "sending" => MessageStatus::Sending,
        "delivered" => MessageStatus::Delivered,
        "read" => MessageStatus::Read,
        "failed" => MessageStatus::Failed,
        "sent" | "" => MessageStatus::Sent,
        _ => MessageStatus::Unknown,
    }
}

impl Store {
    /// Up to `limit` messages of a room, newest last. `before_order` 0 means the latest messages.
    pub fn list_messages(
        &self,
        room_id: &str,
        before_order: i64,
        limit: usize,
    ) -> Result<Vec<Message>> {
        let events = {
            let conn = self.conn();
            let mut stmt = conn.prepare(
                "SELECT event_id, room_id, ord, sender, type, state_key, content_json, ts,
                     COALESCE(txn_id, ''), reply_to, edit_content_json, edited_ts, redacted,
                     status, error, part_index, remote_id
                 FROM event WHERE room_id = ?1 AND (?2 = 0 OR ord < ?2)
                 ORDER BY ord DESC LIMIT ?3",
            )?;
            let rows = stmt.query_map(
                params![room_id, before_order, limit as i64],
                |row| {
                    Ok(Event {
                        event_id: row.get(0)?,
                        room_id: row.get(1)?,
                        order: row.get(2)?,
                        sender: row.get(3)?,
                        kind: row.get(4)?,
                        state_key: row.get(5)?,
                        content_json: row.get(6)?,
                        ts: row.get(7)?,
                        txn_id: row.get(8)?,
                        reply_to: row.get(9)?,
                        edit_json: row.get(10)?,
                        edited_ts: row.get(11)?,
                        redacted: row.get(12)?,
                        status: row.get(13)?,
                        error: row.get(14)?,
                        part_index: row.get(15)?,
                        remote_id: row.get(16)?,
                    })
                },
            )?;
            let mut out = Vec::new();
            for event in rows {
                out.push(event?);
            }
            out
        };
        let mut events = events;
        events.reverse();
        self.render(&events)
    }

    pub fn message_by_id(&self, event_id: &str) -> Result<Message> {
        let event = self.event(event_id)?;
        let mut messages = self.render(&[event])?;
        Ok(messages.remove(0))
    }

    /// Up to `limit` messages whose text contains `query`, newest first.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Message>> {
        let events = {
            let conn = self.conn();
            let mut stmt = conn.prepare(
                "SELECT event_id, room_id, ord, sender, type, state_key, content_json, ts,
                     COALESCE(txn_id, ''), reply_to, edit_content_json, edited_ts, redacted,
                     status, error, part_index, remote_id
                 FROM event
                 WHERE redacted = false AND type = 'message'
                   AND COALESCE(
                         json_extract(edit_content_json, '$.body'),
                         json_extract(content_json, '$.body')
                       ) LIKE ?1 ESCAPE '\\'
                 ORDER BY ord DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![like_pattern(query), limit as i64], |row| {
                Ok(Event {
                    event_id: row.get(0)?,
                    room_id: row.get(1)?,
                    order: row.get(2)?,
                    sender: row.get(3)?,
                    kind: row.get(4)?,
                    state_key: row.get(5)?,
                    content_json: row.get(6)?,
                    ts: row.get(7)?,
                    txn_id: row.get(8)?,
                    reply_to: row.get(9)?,
                    edit_json: row.get(10)?,
                    edited_ts: row.get(11)?,
                    redacted: row.get(12)?,
                    status: row.get(13)?,
                    error: row.get(14)?,
                    part_index: row.get(15)?,
                    remote_id: row.get(16)?,
                })
            })?;
            let mut out = Vec::new();
            for event in rows {
                out.push(event?);
            }
            out
        };
        self.render(&events)
    }

    /// Render a batch of events. Reactions are read once for the whole batch.
    fn render(&self, events: &[Event]) -> Result<Vec<Message>> {
        if events.is_empty() {
            return Ok(Vec::new());
        }
        let targets = events
            .iter()
            .map(|e| e.event_id.clone())
            .collect::<Vec<_>>();
        let reactions = self.reactions_of(&targets)?;
        let mut out = Vec::with_capacity(events.len());
        for event in events {
            let mine = reactions
                .iter()
                .filter(|r| r.target_event_id == event.event_id)
                .cloned()
                .collect::<Vec<_>>();
            out.push(self.render_one(event, &mine)?);
        }
        Ok(out)
    }

    fn render_one(&self, event: &Event, reactions: &[Reaction]) -> Result<Message> {
        let described = if event.redacted {
            Described {
                kind: "unsupported".to_owned(),
                ..Default::default()
            }
        } else {
            content::describe(&event.kind, &event.content_json, event.edit_json.as_deref())
        };
        let (sender_name, sender_avatar) = self.sender_info(&event.room_id, &event.sender);
        let mut message = Message {
            event_id: event.event_id.clone(),
            room_id: event.room_id.clone(),
            order: event.order,
            sender: event.sender.clone(),
            sender_name,
            sender_avatar,
            from_me: event.sender == SELF_USER_ID,
            ts: event.ts,
            kind: described.kind.clone(),
            body: described.body.clone(),
            formatted_body: described.formatted.clone(),
            media: described.media.clone(),
            edited: described.edited,
            redacted: event.redacted,
            status: status_of(event),
            error: event.error.clone(),
            ..Default::default()
        };
        if event.kind == "system" {
            message.kind = "state".to_owned();
        }
        message.reactions = self.aggregate(&event.room_id, reactions);
        if !event.reply_to.is_empty() {
            message.reply_to = Some(self.reply_preview(&event.reply_to));
        }
        Ok(message)
    }

    fn aggregate(&self, room_id: &str, reactions: &[Reaction]) -> Vec<MessageReaction> {
        let mut out: Vec<MessageReaction> = Vec::new();
        let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for reaction in reactions {
            let name = if reaction.sender == SELF_USER_ID {
                "me".to_owned()
            } else {
                self.display_name(room_id, &reaction.sender)
            };
            let at = *index.entry(reaction.key.clone()).or_insert_with(|| {
                out.push(MessageReaction {
                    key: reaction.key.clone(),
                    senders: Vec::new(),
                    ..Default::default()
                });
                out.len() - 1
            });
            out[at].count += 1;
            out[at].senders.push(name);
            out[at].from_me |= reaction.sender == SELF_USER_ID;
        }
        out
    }

    fn reply_preview(&self, target: &str) -> ReplyTo {
        let Ok(event) = self.event(target) else {
            return ReplyTo {
                event_id: target.to_owned(),
                ..Default::default()
            };
        };
        let described = content::describe(&event.kind, &event.content_json, event.edit_json.as_deref());
        ReplyTo {
            event_id: target.to_owned(),
            sender_name: self.display_name(&event.room_id, &event.sender),
            body: described.body,
        }
    }
}

/// The chat-list preview of the last event of a room.
pub(super) fn preview_of(
    event_id: &str,
    sender: &str,
    kind: &str,
    content_json: &str,
    edit_json: Option<&str>,
    sender_name: &str,
) -> Option<ChatPreview> {
    let _ = event_id;
    let described = content::describe(kind, content_json, edit_json);
    Some(ChatPreview {
        sender: if sender_name.is_empty() {
            sender.to_owned()
        } else {
            sender_name.to_owned()
        },
        body: content::preview_text(&described),
        kind: described.kind,
    })
}
