//! Timeline events, edits, reactions and redactions.

use rusqlite::types::Value as SqlValue;
use rusqlite::{OptionalExtension, params};

use super::media::{link_media_tx, remove_media_files, unlink_media_tx};
use super::rooms::{set_backfill_tx, set_profile_tx};
use super::{Error, Profile, Result, Store, now_ms};

/// A stored timeline event.
#[derive(Clone, Debug, Default)]
pub struct Event {
    pub event_id: String,
    pub room_id: String,
    pub order: i64,
    pub sender: String,
    /// `message` for a chat message, `system` for a membership or metadata entry.
    pub kind: String,
    pub state_key: Option<String>,
    pub content_json: String,
    pub ts: i64,
    pub txn_id: String,
    pub reply_to: String,
    pub edit_json: Option<String>,
    pub edited_ts: i64,
    pub redacted: bool,
    pub status: String,
    pub error: String,
    pub part_index: i64,
    /// The network's own id of this event, for addressing it from a driver.
    pub remote_id: String,
}

/// A stored reaction.
#[derive(Clone, Debug, Default)]
pub struct Reaction {
    pub event_id: String,
    pub room_id: String,
    pub target_event_id: String,
    pub sender: String,
    pub key: String,
    pub ts: i64,
}

/// What a redaction removed.
#[derive(Clone, Debug, Default)]
pub struct Redaction {
    pub room_id: String,
    pub event_id: String,
    pub target_event_id: String,
    pub is_reaction: bool,
}

const EVENT_COLUMNS: &str = "event_id, room_id, ord, sender, type, state_key, content_json, ts,
    COALESCE(txn_id, ''), reply_to, edit_content_json, edited_ts, redacted, status, error,
    part_index, remote_id";

fn scan_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<Event> {
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
}

/// Order slots per millisecond: events with the same timestamp keep their insertion order.
const ORDER_BUCKET: i64 = 1000;

impl Store {
    /// Store a visible timeline event, idempotent on the event id. Returns whether it was inserted;
    /// `e.order` is always set to the assigned (or existing) order key.
    pub fn insert_event(&self, e: &mut Event) -> Result<bool> {
        self.write(|tx| insert_event_tx(tx, e))
    }

    /// Store one page of history, its profiles and its cursor in one transaction, so a page costs
    /// one commit instead of one per message. Returns how many events were new.
    pub fn store_backfill_page(
        &self,
        room_id: &str,
        events: &mut [Event],
        profiles: &[Profile],
        cursor: &str,
        done: bool,
    ) -> Result<usize> {
        self.write(|tx| {
            for profile in profiles {
                set_profile_tx(tx, profile)?;
            }
            let mut added = 0;
            for event in events.iter_mut() {
                if insert_event_tx(tx, event)? {
                    added += 1;
                }
            }
            set_backfill_tx(tx, room_id, cursor, done)?;
            Ok(added)
        })
    }

    pub fn event(&self, event_id: &str) -> Result<Event> {
        let conn = self.conn();
        conn.query_row(
            &format!("SELECT {EVENT_COLUMNS} FROM event WHERE event_id = ?1"),
            params![event_id],
            scan_event,
        )
        .optional()?
        .ok_or(Error::NotFound)
    }

    /// The stored event of a chat with a given remote id.
    pub fn event_by_remote(&self, room_id: &str, remote_id: &str) -> Result<Option<Event>> {
        let conn = self.conn();
        Ok(conn
            .query_row(
                &sql_event_by_remote(),
                params![room_id, remote_id],
                scan_event,
            )
            .optional()?)
    }

    /// Events this device sent that the network has not linked to a remote message yet.
    pub fn pending_outgoing(&self, room_id: &str) -> Result<Vec<Event>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {EVENT_COLUMNS} FROM event
             WHERE room_id = ?1 AND status IN ('sending', 'sent') AND COALESCE(remote_id, '') = ''
               AND redacted = false ORDER BY ord"
        ))?;
        let rows = stmt.query_map(params![room_id], scan_event)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn event_by_txn(&self, room_id: &str, txn_id: &str) -> Result<Option<Event>> {
        let conn = self.conn();
        Ok(conn
            .query_row(&sql_event_by_txn(), params![room_id, txn_id], scan_event)
            .optional()?)
    }

    /// Store the new content of an edit on the target event.
    pub fn apply_edit(
        &self,
        room_id: &str,
        target: &str,
        new_content_json: &str,
        ts: i64,
    ) -> Result<()> {
        let conn = self.conn();
        let changed = conn.execute(
            "UPDATE event SET edit_content_json = ?3, edited_ts = ?4
             WHERE room_id = ?1 AND event_id = ?2 AND redacted = false",
            params![room_id, target, new_content_json, ts],
        )?;
        if changed == 0 {
            return Err(Error::NotFound);
        }
        Ok(())
    }

    /// Redact a message or delete a stored reaction named by `event_id`.
    pub fn redact(&self, event_id: &str) -> Result<Redaction> {
        self.write(|tx| {
            let reaction = tx
                .query_row(
                    "SELECT room_id, target_event_id FROM reaction WHERE event_id = ?1",
                    params![event_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?;
            if let Some((room_id, target_event_id)) = reaction {
                tx.execute(
                    "DELETE FROM reaction WHERE event_id = ?1",
                    params![event_id],
                )?;
                return Ok(Redaction {
                    room_id,
                    event_id: event_id.to_owned(),
                    target_event_id,
                    is_reaction: true,
                });
            }
            let room_id: String = tx
                .query_row(
                    "SELECT room_id FROM event WHERE event_id = ?1",
                    params![event_id],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or(Error::NotFound)?;
            tx.execute(
                "UPDATE event SET redacted = true, content_json = '{}', edit_content_json = NULL,
                     error = '' WHERE event_id = ?1",
                params![event_id],
            )?;
            Ok(Redaction {
                room_id,
                event_id: event_id.to_owned(),
                target_event_id: event_id.to_owned(),
                is_reaction: false,
            })
        })
    }

    /// Delete the reaction stored under `event_id` and return it.
    pub fn remove_reaction_by_event_id(&self, event_id: &str) -> Result<Reaction> {
        let (room_id, target): (String, String) = {
            let conn = self.conn();
            conn.query_row(
                "SELECT room_id, target_event_id FROM reaction WHERE event_id = ?1",
                params![event_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or(Error::NotFound)?
        };
        let r = Reaction {
            event_id: event_id.to_owned(),
            room_id,
            target_event_id: target,
            ..Default::default()
        };
        let conn = self.conn();
        conn.execute(
            "DELETE FROM reaction WHERE event_id = ?1",
            params![event_id],
        )?;
        Ok(r)
    }

    /// Store a reaction once per sender and key.
    pub fn add_reaction(&self, r: &Reaction) -> Result<bool> {
        let inserted = self.write(|tx| {
            let changed = tx.execute(
                "INSERT INTO reaction (event_id, room_id, target_event_id, sender, key, ts)
                 SELECT ?1, ?2, ?3, ?4, ?5, ?6
                 WHERE NOT EXISTS (
                     SELECT 1 FROM reaction WHERE target_event_id = ?3 AND sender = ?4 AND key = ?5
                 ) ON CONFLICT (event_id) DO NOTHING",
                params![
                    r.event_id,
                    r.room_id,
                    r.target_event_id,
                    r.sender,
                    r.key,
                    r.ts
                ],
            )?;
            Ok(changed > 0)
        })?;
        Ok(inserted)
    }

    /// Replace the whole reaction set of a message, which is what a driver reports.
    pub fn set_reactions(
        &self,
        room_id: &str,
        target: &str,
        reactions: &[(String, String)],
    ) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "DELETE FROM reaction WHERE target_event_id = ?1",
                params![target],
            )?;
            for (sender, key) in reactions {
                let event_id = format!(
                    "$rx.{}",
                    chirper_bridge::ids::hash_hex(&[room_id, target, sender, key])
                );
                tx.execute(
                    "INSERT OR IGNORE INTO reaction (event_id, room_id, target_event_id, sender, key, ts)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![event_id, room_id, target, sender, key, now_ms()],
                )?;
            }
            Ok(())
        })
    }

    /// Replace the reaction set of a message with the network's summary. When the network does not
    /// say who reacted, the rows carry no sender and only the counts are shown.
    pub fn set_reaction_summary(
        &self,
        room_id: &str,
        target: &str,
        summary: &[(String, i64, bool, Vec<String>)],
    ) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "DELETE FROM reaction WHERE target_event_id = ?1",
                params![target],
            )?;
            let ts = now_ms();
            for (key, count, from_me, senders) in summary {
                if !senders.is_empty() {
                    for sender in senders {
                        let event_id = format!(
                            "$rx.{}",
                            chirper_bridge::ids::hash_hex(&[room_id, target, sender, key])
                        );
                        tx.execute(
                            "INSERT OR IGNORE INTO reaction
                                 (event_id, room_id, target_event_id, sender, key, ts)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                            params![event_id, room_id, target, sender, key, ts],
                        )?;
                    }
                    continue;
                }
                for index in 0..(*count).max(0) {
                    let sender = if *from_me && index == 0 {
                        crate::ids::SELF_USER_ID
                    } else {
                        ""
                    };
                    let event_id = format!(
                        "$rx.{}",
                        chirper_bridge::ids::hash_hex(&[room_id, target, key, &index.to_string()])
                    );
                    tx.execute(
                        "INSERT OR IGNORE INTO reaction
                             (event_id, room_id, target_event_id, sender, key, ts)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![event_id, room_id, target, sender, key, ts],
                    )?;
                }
            }
            Ok(())
        })
    }

    pub fn find_reactions(&self, target: &str, sender: &str) -> Result<Vec<Reaction>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT event_id, room_id, target_event_id, sender, key, ts FROM reaction
             WHERE target_event_id = ?1 AND sender = ?2 ORDER BY ts, event_id",
        )?;
        let rows = stmt.query_map(params![target, sender], scan_reaction)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// The reactions of several messages, in stored order.
    pub fn reactions_of(&self, targets: &[String]) -> Result<Vec<Reaction>> {
        if targets.is_empty() {
            return Ok(Vec::new());
        }
        let marks = (1..=targets.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let args = targets
            .iter()
            .map(|t| SqlValue::Text(t.clone()))
            .collect::<Vec<_>>();
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT event_id, room_id, target_event_id, sender, key, ts FROM reaction
             WHERE target_event_id IN ({marks}) ORDER BY ts, event_id"
        ))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), scan_reaction)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Update the send status of an outgoing event. Statuses only move forward: sending, sent,
    /// delivered, read. A late "delivered" never downgrades a "read", and a late "sent" never
    /// downgrades a "delivered". `failed` is outside that order: it replaces an unconfirmed send
    /// but never a delivered or read one, and only a later success (sent, delivered or read)
    /// replaces it.
    pub fn set_event_status(&self, event_id: &str, status: &str, error: &str) -> Result<()> {
        const UNCONFIRMED: &str = "('sending', 'sent', 'failed')";
        const SUCCESS: &str = "('sent', 'delivered', 'read')";
        let conn = self.conn();
        conn.execute(
            &format!(
                "UPDATE event SET
                     status = CASE
                         WHEN ?2 = 'failed' THEN
                             CASE WHEN status IN {UNCONFIRMED} THEN 'failed' ELSE status END
                         WHEN status = 'failed' THEN
                             CASE WHEN ?2 IN {SUCCESS} THEN ?2 ELSE status END
                         WHEN {} >= {} THEN ?2
                         ELSE status END,
                     error = CASE
                         WHEN ?2 = 'failed' AND status IN {UNCONFIRMED} THEN ?3
                         WHEN status = 'failed' AND ?2 IN {SUCCESS} THEN ''
                         ELSE error END
                 WHERE event_id = ?1",
                status_rank("?2"),
                status_rank("status"),
            ),
            params![event_id, status, error],
        )?;
        Ok(())
    }

    /// Remember the network's id of an event that was sent locally.
    pub fn set_event_remote(&self, event_id: &str, remote_id: &str) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "UPDATE event SET remote_id = ?2 WHERE event_id = ?1",
            params![event_id, remote_id],
        )?;
        Ok(())
    }

    /// Put back the content columns of a snapshot, undoing a rejected optimistic write.
    pub fn restore_event(&self, snap: &Event) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "UPDATE event SET content_json = ?2, edit_content_json = ?3, edited_ts = ?4,
                 redacted = ?5 WHERE event_id = ?1",
            params![
                snap.event_id,
                snap.content_json,
                snap.edit_json,
                snap.edited_ts,
                snap.redacted
            ],
        )?;
        Ok(())
    }

    /// Delete the media links of an event and any media row left unreferenced.
    pub fn release_event_media(&self, event_id: &str) -> Result<()> {
        let paths = self.write(|tx| {
            unlink_media_tx(tx, "event_id = ?1", &[SqlValue::Text(event_id.to_owned())])
        })?;
        remove_media_files(&paths);
        Ok(())
    }
}

fn scan_reaction(row: &rusqlite::Row<'_>) -> rusqlite::Result<Reaction> {
    Ok(Reaction {
        event_id: row.get(0)?,
        room_id: row.get(1)?,
        target_event_id: row.get(2)?,
        sender: row.get(3)?,
        key: row.get(4)?,
        ts: row.get(5)?,
    })
}

/// The SQL rank of a send status in the forward order (`failed` ranks below every other status).
fn status_rank(column: &str) -> String {
    format!(
        "CASE {column} WHEN 'sending' THEN 0 WHEN 'sent' THEN 1 WHEN 'delivered' THEN 2 \
         WHEN 'read' THEN 3 ELSE -1 END"
    )
}

fn null_if_empty(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

fn next_order_tx(tx: &rusqlite::Transaction<'_>, room_id: &str, ts: i64) -> Result<i64> {
    let base = ts * ORDER_BUCKET;
    let max: i64 = tx.query_row(
        "SELECT COALESCE(MAX(ord), -1) FROM event WHERE room_id = ?1 AND ord >= ?2 AND ord < ?3",
        params![room_id, base, base + ORDER_BUCKET],
        |row| row.get(0),
    )?;
    Ok(if max < base {
        base
    } else {
        (max + 1).min(base + ORDER_BUCKET - 1)
    })
}

/// Move a chat in the list only when a message arrives: state changes and reactions leave it where
/// its last message put it.
fn touch_room_tx(tx: &rusqlite::Transaction<'_>, e: &Event) -> Result<()> {
    if e.state_key.is_some() || e.kind != "message" {
        return Ok(());
    }
    tx.execute(
        "UPDATE room SET
             last_event_id = CASE WHEN ?2 >= last_activity_ts THEN ?3 ELSE last_event_id END,
             last_activity_ts = MAX(last_activity_ts, ?2)
         WHERE room_id = ?1",
        params![e.room_id, e.ts, e.event_id],
    )?;
    Ok(())
}

/// Insert one event inside an open transaction; see [`Store::insert_event`].
fn insert_event_tx(tx: &rusqlite::Transaction<'_>, e: &mut Event) -> Result<bool> {
    let existing: Option<i64> = tx
        .query_row(
            "SELECT ord FROM event WHERE event_id = ?1",
            params![e.event_id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(order) = existing {
        e.order = order;
        return Ok(false);
    }
    e.order = next_order_tx(tx, &e.room_id, e.ts)?;
    let changed = tx.execute(
        "INSERT INTO event (event_id, room_id, ord, sender, type, state_key, content_json,
                 ts, txn_id, reply_to, status, error, part_index, remote_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT DO NOTHING",
        params![
            e.event_id,
            e.room_id,
            e.order,
            e.sender,
            e.kind,
            e.state_key,
            e.content_json,
            e.ts,
            null_if_empty(&e.txn_id),
            e.reply_to,
            e.status,
            e.error,
            e.part_index,
            e.remote_id,
        ],
    )?;
    if changed == 0 {
        // Another row with the same transaction id exists under a different event id.
        return Ok(false);
    }
    link_media_tx(tx, &e.event_id, &e.room_id, &e.content_json)?;
    touch_room_tx(tx, e)?;
    Ok(true)
}

/// The lookup of an event by its remote id. A function so the query-plan test runs this exact text.
pub(super) fn sql_event_by_remote() -> String {
    // `INDEXED BY` pins the remote-id index. Without it the planner walks the room's events in
    // `ord` order, because of the ORDER BY, and reads the whole room before it finds a match.
    format!(
        "SELECT {EVENT_COLUMNS} FROM event INDEXED BY event_remote_idx
         WHERE room_id = ?1 AND remote_id = ?2 ORDER BY ord LIMIT 1"
    )
}

/// The lookup of an event by the transaction id of an outgoing message.
pub(super) fn sql_event_by_txn() -> String {
    format!("SELECT {EVENT_COLUMNS} FROM event WHERE room_id = ?1 AND txn_id = ?2")
}
