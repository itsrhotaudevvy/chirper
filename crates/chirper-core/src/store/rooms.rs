//! Rooms, chats, members, profiles and logins.

use rusqlite::types::Value as SqlValue;
use rusqlite::{OptionalExtension, params, params_from_iter};

use super::media::unlink_media_tx;
use super::{Error, Login, Result, Store, like_pattern, now_ms};
use crate::model::{Chat, ChatFilter};

/// A room row, including the remote identity of the chat.
#[derive(Clone, Debug, Default)]
pub struct Room {
    pub room_id: String,
    pub bridge_id: String,
    pub login_id: String,
    pub portal_id: String,
    pub remote_id: String,
    pub name: String,
    pub topic: String,
    pub avatar_url: String,
    pub is_dm: bool,
    pub is_space: bool,
    pub dm_user_id: String,
    pub created_ts: i64,
    pub last_activity_ts: i64,
    pub last_event_id: String,
    pub unread_count: i64,
    pub mention_count: i64,
    pub read_up_to_order: i64,
    pub pinned: bool,
    pub archived: bool,
    pub muted_until: i64,
    pub tags_json: String,
    pub deleted: bool,
    pub parent_id: String,
    pub parent_remote_id: String,
}

/// The room fields that can be changed one at a time. Absent fields are left unchanged.
#[derive(Clone, Debug, Default)]
pub struct RoomFields {
    pub login_id: Option<String>,
    pub portal_id: Option<String>,
    pub remote_id: Option<String>,
    pub name: Option<String>,
    pub topic: Option<String>,
    pub avatar_url: Option<String>,
    pub pinned: Option<bool>,
    pub archived: Option<bool>,
    pub muted_until: Option<i64>,
    pub tags_json: Option<String>,
    pub parent_id: Option<String>,
    pub parent_remote_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Member {
    pub room_id: String,
    pub user_id: String,
    pub displayname: String,
    pub avatar_url: String,
    pub membership: String,
}

#[derive(Clone, Debug, Default)]
pub struct Profile {
    pub user_id: String,
    pub displayname: String,
    pub avatar_url: String,
    pub extra_json: String,
}

const ROOM_COLUMNS: &str = "room_id, bridge_id, login_id, portal_id, remote_id, name, topic, avatar_url, \
     is_dm, is_space, dm_user_id, created_ts, last_activity_ts, last_event_id, unread_count, \
     mention_count, read_up_to_order, pinned, archived, muted_until, tags_json, deleted, \
     parent_id, parent_remote_id";

fn scan_room(row: &rusqlite::Row<'_>) -> rusqlite::Result<Room> {
    Ok(Room {
        room_id: row.get(0)?,
        bridge_id: row.get(1)?,
        login_id: row.get(2)?,
        portal_id: row.get(3)?,
        remote_id: row.get(4)?,
        name: row.get(5)?,
        topic: row.get(6)?,
        avatar_url: row.get(7)?,
        is_dm: row.get(8)?,
        is_space: row.get(9)?,
        dm_user_id: row.get(10)?,
        created_ts: row.get(11)?,
        last_activity_ts: row.get(12)?,
        last_event_id: row.get(13)?,
        unread_count: row.get(14)?,
        mention_count: row.get(15)?,
        read_up_to_order: row.get(16)?,
        pinned: row.get(17)?,
        archived: row.get(18)?,
        muted_until: row.get(19)?,
        tags_json: row.get(20)?,
        deleted: row.get(21)?,
        parent_id: row.get(22)?,
        parent_remote_id: row.get(23)?,
    })
}

impl Store {
    /// Create the room or update its identity columns. Name, topic, avatar and user flags are left
    /// alone; use [`Store::set_room_fields`] for those.
    pub fn upsert_room(&self, room: &Room) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO room (room_id, bridge_id, login_id, portal_id, remote_id, is_dm, is_space,
                 dm_user_id, created_ts, deleted, parent_id, parent_remote_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, false, ?10, ?11)
             ON CONFLICT (room_id) DO UPDATE SET
                 bridge_id = excluded.bridge_id,
                 login_id = COALESCE(NULLIF(excluded.login_id, ''), room.login_id),
                 portal_id = COALESCE(NULLIF(excluded.portal_id, ''), room.portal_id),
                 remote_id = COALESCE(NULLIF(excluded.remote_id, ''), room.remote_id),
                 is_dm = excluded.is_dm,
                 is_space = excluded.is_space,
                 dm_user_id = excluded.dm_user_id,
                 deleted = false,
                 parent_id = COALESCE(NULLIF(excluded.parent_id, ''), room.parent_id),
                 parent_remote_id = COALESCE(NULLIF(excluded.parent_remote_id, ''), room.parent_remote_id)",
            params![
                room.room_id,
                room.bridge_id,
                room.login_id,
                room.portal_id,
                room.remote_id,
                room.is_dm,
                room.is_space,
                room.dm_user_id,
                room.created_ts,
                room.parent_id,
                room.parent_remote_id,
            ],
        )?;
        Ok(())
    }

    /// The room row, including deleted rooms.
    pub fn room(&self, room_id: &str) -> Result<Room> {
        let conn = self.conn();
        conn.query_row(
            &format!("SELECT {ROOM_COLUMNS} FROM room WHERE room_id = ?1"),
            params![room_id],
            scan_room,
        )
        .optional()?
        .ok_or(Error::NotFound)
    }

    /// The room row of a remote chat, if it is known.
    pub fn room_by_remote(&self, network: &str, remote_id: &str) -> Result<Option<Room>> {
        let conn = self.conn();
        Ok(conn
            .query_row(
                &format!(
                    "SELECT {ROOM_COLUMNS} FROM room
                     WHERE bridge_id = ?1 AND remote_id = ?2 AND deleted = false LIMIT 1"
                ),
                params![network, remote_id],
                scan_room,
            )
            .optional()?)
    }

    /// A room by the portal id the Go core stored.
    pub fn room_by_portal(&self, network: &str, portal_id: &str) -> Result<Option<Room>> {
        let conn = self.conn();
        Ok(conn
            .query_row(
                &format!(
                    "SELECT {ROOM_COLUMNS} FROM room
                     WHERE bridge_id = ?1 AND portal_id = ?2 AND deleted = false LIMIT 1"
                ),
                params![network, portal_id],
                scan_room,
            )
            .optional()?)
    }

    /// Apply the non-`None` fields.
    pub fn set_room_fields(&self, room_id: &str, f: &RoomFields) -> Result<()> {
        let mut sets: Vec<String> = Vec::new();
        let mut args: Vec<SqlValue> = Vec::new();
        let mut add = |column: &str, value: SqlValue| {
            args.push(value);
            sets.push(format!("{column} = ?{}", args.len()));
        };
        if let Some(v) = &f.login_id {
            add("login_id", SqlValue::Text(v.clone()));
        }
        if let Some(v) = &f.portal_id {
            add("portal_id", SqlValue::Text(v.clone()));
        }
        if let Some(v) = &f.remote_id {
            add("remote_id", SqlValue::Text(v.clone()));
        }
        if let Some(v) = &f.name {
            add("name", SqlValue::Text(v.clone()));
        }
        if let Some(v) = &f.topic {
            add("topic", SqlValue::Text(v.clone()));
        }
        if let Some(v) = &f.avatar_url {
            add("avatar_url", SqlValue::Text(v.clone()));
        }
        if let Some(v) = f.pinned {
            add("pinned", SqlValue::Integer(v as i64));
        }
        if let Some(v) = f.archived {
            add("archived", SqlValue::Integer(v as i64));
        }
        if let Some(v) = f.muted_until {
            add("muted_until", SqlValue::Integer(v));
        }
        if let Some(v) = &f.tags_json {
            add("tags_json", SqlValue::Text(v.clone()));
        }
        if let Some(v) = &f.parent_id {
            add("parent_id", SqlValue::Text(v.clone()));
        }
        if let Some(v) = &f.parent_remote_id {
            add("parent_remote_id", SqlValue::Text(v.clone()));
        }
        if sets.is_empty() {
            return Ok(());
        }
        args.push(SqlValue::Text(room_id.to_owned()));
        let conn = self.conn();
        conn.execute(
            &format!(
                "UPDATE room SET {} WHERE room_id = ?{}",
                sets.join(", "),
                args.len()
            ),
            params_from_iter(args.iter()),
        )?;
        Ok(())
    }

    /// Mark the room deleted and erase its content. The row stays, hidden from listings, so a later
    /// rejoin can reuse it.
    pub fn delete_room(&self, room_id: &str) -> Result<Vec<String>> {
        let (paths, _) = self.write(|tx| {
            let paths = unlink_media_tx(tx, "room_id = ?1", &[SqlValue::Text(room_id.to_owned())])?;
            tx.execute("DELETE FROM event WHERE room_id = ?1", params![room_id])?;
            tx.execute("DELETE FROM member WHERE room_id = ?1", params![room_id])?;
            tx.execute("DELETE FROM backfill WHERE room_id = ?1", params![room_id])?;
            tx.execute(
                "UPDATE room SET deleted = true, name = '', topic = '', avatar_url = '',
                     last_event_id = '', unread_count = 0, mention_count = 0, read_up_to_order = 0
                 WHERE room_id = ?1",
                params![room_id],
            )?;
            Ok((paths, ()))
        })?;
        Ok(paths)
    }

    /// Read the chats matching a filter, pinned first, then most recently active.
    pub fn list_chats(&self, filter: &ChatFilter) -> Result<Vec<Chat>> {
        let mut query = format!(
            "{CHAT_SELECT} WHERE room.deleted = false AND room.is_space = false AND room.archived = ?1"
        );
        let mut args: Vec<SqlValue> = vec![SqlValue::Integer(filter.archived as i64)];
        if !filter.network.is_empty() {
            args.push(SqlValue::Text(filter.network.clone()));
            query.push_str(&format!(" AND room.bridge_id = ?{}", args.len()));
        }
        if filter.direct_only {
            query.push_str(" AND (room.parent_id = '' OR room.is_dm = true)");
        } else if filter.servers_only {
            query.push_str(" AND room.parent_id <> '' AND room.is_dm = false");
        }
        if filter.unread_only {
            query.push_str(" AND room.unread_count > 0");
        }
        if !filter.query.is_empty() {
            args.push(SqlValue::Text(like_pattern(&filter.query)));
            query.push_str(&format!(
                " AND (room.name LIKE ?{n} ESCAPE '\\' OR {PARENT_NAME} LIKE ?{n} ESCAPE '\\')",
                n = args.len(),
                PARENT_NAME = PARENT_NAME
            ));
        }
        query.push_str(" ORDER BY room.pinned DESC, room.last_activity_ts DESC, room.room_id");

        let conn = self.conn();
        let mut stmt = conn.prepare(&query)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), scan_chat)?;
        let mut chats = Vec::new();
        for chat in rows {
            chats.push(chat?);
        }
        Ok(chats)
    }

    /// One chat, or [`Error::NotFound`].
    pub fn chat(&self, room_id: &str) -> Result<Chat> {
        let conn = self.conn();
        conn.query_row(
            &format!("{CHAT_SELECT} WHERE room.room_id = ?1"),
            params![room_id],
            scan_chat,
        )
        .optional()?
        .ok_or(Error::NotFound)
    }

    /// A space room by its remote (portal) id.
    pub fn space_room(&self, bridge_id: &str, portal_id: &str) -> Result<Option<Room>> {
        let conn = self.conn();
        Ok(conn
            .query_row(
                &format!(
                    "SELECT {ROOM_COLUMNS} FROM room
                     WHERE bridge_id = ?1 AND (portal_id = ?2 OR remote_id = ?2)
                       AND is_space = true AND deleted = false LIMIT 1"
                ),
                params![bridge_id, portal_id],
                scan_room,
            )
            .optional()?)
    }

    pub fn set_membership(&self, room_id: &str, user_id: &str, membership: &str) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO member (room_id, user_id, displayname, avatar_url, membership)
             VALUES (?1, ?2, '', '', ?3)
             ON CONFLICT (room_id, user_id) DO UPDATE SET membership = excluded.membership",
            params![room_id, user_id, membership],
        )?;
        Ok(())
    }

    pub fn upsert_member(&self, m: &Member) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO member (room_id, user_id, displayname, avatar_url, membership)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (room_id, user_id) DO UPDATE SET
                 displayname = excluded.displayname,
                 avatar_url = excluded.avatar_url,
                 membership = excluded.membership",
            params![m.room_id, m.user_id, m.displayname, m.avatar_url, m.membership],
        )?;
        Ok(())
    }

    pub fn member(&self, room_id: &str, user_id: &str) -> Result<Member> {
        let conn = self.conn();
        conn.query_row(
            "SELECT room_id, user_id, displayname, avatar_url, membership
             FROM member WHERE room_id = ?1 AND user_id = ?2",
            params![room_id, user_id],
            |row| {
                Ok(Member {
                    room_id: row.get(0)?,
                    user_id: row.get(1)?,
                    displayname: row.get(2)?,
                    avatar_url: row.get(3)?,
                    membership: row.get(4)?,
                })
            },
        )
        .optional()?
        .ok_or(Error::NotFound)
    }

    pub fn profile(&self, user_id: &str) -> Result<Profile> {
        let conn = self.conn();
        conn.query_row(
            "SELECT user_id, displayname, avatar_url, extra_json FROM profile WHERE user_id = ?1",
            params![user_id],
            |row| {
                Ok(Profile {
                    user_id: row.get(0)?,
                    displayname: row.get(1)?,
                    avatar_url: row.get(2)?,
                    extra_json: row.get(3)?,
                })
            },
        )
        .optional()?
        .ok_or(Error::NotFound)
    }

    /// Store a global profile and copy its name and avatar onto the user's member rows.
    pub fn set_profile(&self, p: &Profile) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO profile (user_id, displayname, avatar_url, extra_json)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (user_id) DO UPDATE SET
                     displayname = excluded.displayname,
                     avatar_url = excluded.avatar_url,
                     extra_json = excluded.extra_json",
                params![p.user_id, p.displayname, p.avatar_url, p.extra_json],
            )?;
            tx.execute(
                "UPDATE member SET displayname = ?2, avatar_url = ?3 WHERE user_id = ?1",
                params![p.user_id, p.displayname, p.avatar_url],
            )?;
            Ok(())
        })
    }

    /// The name shown for a user in a room: member name, then profile name, then the id.
    pub fn display_name(&self, room_id: &str, user_id: &str) -> String {
        let (name, _) = self.sender_info(room_id, user_id);
        name
    }

    /// The display name and avatar of a user in a room.
    pub fn sender_info(&self, room_id: &str, user_id: &str) -> (String, String) {
        let mut name = String::new();
        let mut avatar = String::new();
        if let Ok(member) = self.member(room_id, user_id) {
            name = member.displayname;
            avatar = member.avatar_url;
        }
        if name.is_empty() || avatar.is_empty() {
            if let Ok(profile) = self.profile(user_id) {
                if name.is_empty() {
                    name = profile.displayname;
                }
                if avatar.is_empty() {
                    avatar = profile.avatar_url;
                }
            }
        }
        if name.is_empty() {
            name = user_id.to_owned();
        }
        (name, avatar)
    }

    /// Move the read marker and recompute the unread count.
    pub fn mark_read_up_to(&self, room_id: &str, event_id: &str) -> Result<()> {
        self.write(|tx| {
            let ord: i64 = tx
                .query_row(
                    "SELECT ord FROM event WHERE room_id = ?1 AND event_id = ?2",
                    params![room_id, event_id],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or(Error::NotFound)?;
            tx.execute(
                "UPDATE room SET read_up_to_order = ?2, unread_count = (
                     SELECT COUNT(*) FROM event
                     WHERE event.room_id = ?1 AND event.ord > ?2 AND event.sender <> ?3
                       AND event.state_key IS NULL AND event.redacted = false
                 ) WHERE room_id = ?1",
                params![room_id, ord, crate::ids::SELF_USER_ID],
            )?;
            Ok(())
        })
    }

    /// Mark a room unread, or read when `unread` is false.
    pub fn mark_unread(&self, room_id: &str, unread: bool) -> Result<()> {
        if unread {
            let conn = self.conn();
            conn.execute(
                "UPDATE room SET unread_count = MAX(unread_count, 1) WHERE room_id = ?1",
                params![room_id],
            )?;
            return Ok(());
        }
        let room = self.room(room_id)?;
        if room.last_event_id.is_empty() {
            return Ok(());
        }
        self.mark_read_up_to(room_id, &room.last_event_id)
    }

    /// Count an inserted incoming message as unread when it is newer than the read marker.
    pub fn increment_unread(&self, event_id: &str) -> Result<()> {
        let ev = self.event(event_id)?;
        if ev.sender == crate::ids::SELF_USER_ID || ev.state_key.is_some() {
            return Ok(());
        }
        let conn = self.conn();
        conn.execute(
            "UPDATE room SET unread_count = unread_count + 1
             WHERE room_id = ?1 AND read_up_to_order < ?2",
            params![ev.room_id, ev.order],
        )?;
        Ok(())
    }

    /// Store a login row (used for accounts that are not connected right now).
    pub fn put_login(&self, login: &Login) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO login (network, login_id, name, avatar_url, remote_id, added_ts)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (network, login_id) DO UPDATE SET
                 name = COALESCE(NULLIF(excluded.name, ''), login.name),
                 avatar_url = COALESCE(NULLIF(excluded.avatar_url, ''), login.avatar_url),
                 remote_id = COALESCE(NULLIF(excluded.remote_id, ''), login.remote_id)",
            params![
                login.network,
                login.login_id,
                login.name,
                login.avatar_url,
                login.remote_id,
                if login.added_ts == 0 {
                    now_ms()
                } else {
                    login.added_ts
                },
            ],
        )?;
        Ok(())
    }

    pub fn delete_login(&self, network: &str, login_id: &str) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM login WHERE network = ?1 AND login_id = ?2",
            params![network, login_id],
        )?;
        Ok(())
    }

    /// Every known login, newest first.
    pub fn logins(&self) -> Result<Vec<Login>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT network, login_id, name, avatar_url, remote_id, added_ts FROM login
             ORDER BY added_ts, network, login_id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Login {
                network: row.get(0)?,
                login_id: row.get(1)?,
                name: row.get(2)?,
                avatar_url: row.get(3)?,
                remote_id: row.get(4)?,
                added_ts: row.get(5)?,
            })
        })?;
        let mut out = Vec::new();
        for login in rows {
            out.push(login?);
        }
        Ok(out)
    }

    /// Whether the network has any login stored.
    pub fn has_logins(&self, network: &str) -> Result<bool> {
        let conn = self.conn();
        Ok(conn
            .query_row(
                "SELECT 1 FROM login WHERE network = ?1 LIMIT 1",
                params![network],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Delete every row of a login: rooms, events, reactions, members, media and the login itself.
    /// Returns the rooms that were purged so their chats can be reported removed.
    pub fn purge_login(&self, network: &str, login_id: &str) -> Result<Vec<String>> {
        let rooms = {
            let conn = self.conn();
            let mut stmt = conn.prepare(
                "SELECT room_id FROM room WHERE bridge_id = ?1 AND login_id = ?2",
            )?;
            let rows = stmt.query_map(params![network, login_id], |row| row.get::<_, String>(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut paths = Vec::new();
        for room_id in &rooms {
            paths.extend(self.delete_room(room_id)?);
        }
        self.delete_login(network, login_id)?;
        Ok(rooms)
    }

    /// The backfill cursor of a room.
    pub fn backfill(&self, room_id: &str) -> Result<(String, bool)> {
        let conn = self.conn();
        Ok(conn
            .query_row(
                "SELECT cursor, done FROM backfill WHERE room_id = ?1",
                params![room_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .unwrap_or_default())
    }

    pub fn set_backfill(&self, room_id: &str, cursor: &str, done: bool) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO backfill (room_id, cursor, done) VALUES (?1, ?2, ?3)
             ON CONFLICT (room_id) DO UPDATE SET cursor = excluded.cursor, done = excluded.done",
            params![room_id, cursor, done],
        )?;
        Ok(())
    }
}

/// The parent's display name, used by both the chat list and its search filter.
const PARENT_NAME: &str = "COALESCE((
    SELECT name FROM room p WHERE p.room_id = room.parent_id AND p.deleted = false LIMIT 1
), '')";

const CHAT_SELECT: &str = "SELECT room.room_id, room.bridge_id, room.login_id, room.portal_id,
    room.remote_id, room.name, room.topic, room.avatar_url, room.is_dm, room.is_space,
    room.dm_user_id, room.created_ts, room.last_activity_ts, room.last_event_id,
    room.unread_count, room.mention_count, room.read_up_to_order, room.pinned, room.archived,
    room.muted_until, room.tags_json, room.deleted, room.parent_id, room.parent_remote_id,
    (SELECT COUNT(*) FROM member WHERE member.room_id = room.room_id AND member.membership = 'join'),
    ev.event_id, ev.sender, ev.type, ev.content_json, ev.edit_content_json,
    COALESCE(NULLIF(em.displayname, ''), NULLIF(pr.displayname, '')),
    (SELECT name FROM room p WHERE p.room_id = room.parent_id AND p.deleted = false LIMIT 1)
    FROM room
    LEFT JOIN event ev ON ev.event_id = room.last_event_id
    LEFT JOIN member em ON em.room_id = ev.room_id AND em.user_id = ev.sender
    LEFT JOIN profile pr ON pr.user_id = ev.sender";

fn scan_chat(row: &rusqlite::Row<'_>) -> rusqlite::Result<Chat> {
    let room = scan_room(row)?;
    let member_count: i64 = row.get(24)?;
    let ev_id: Option<String> = row.get(25)?;
    let ev_sender: Option<String> = row.get(26)?;
    let ev_type: Option<String> = row.get(27)?;
    let ev_content: Option<String> = row.get(28)?;
    let ev_edit: Option<String> = row.get(29)?;
    let sender_name: Option<String> = row.get(30)?;
    let parent_name: Option<String> = row.get(31)?;

    let mut chat = Chat {
        room_id: room.room_id.clone(),
        network: room.bridge_id.clone(),
        login_id: room.login_id.clone(),
        name: room.name,
        avatar_url: room.avatar_url,
        topic: room.topic,
        is_dm: room.is_dm,
        is_space: room.is_space,
        member_count,
        last_activity: room.last_activity_ts,
        unread_count: room.unread_count,
        mention_count: room.mention_count,
        pinned: room.pinned,
        archived: room.archived,
        muted_until: room.muted_until,
        parent_id: room.parent_id.clone(),
        is_channel: !room.parent_id.is_empty(),
        is_group: !room.is_dm && !room.is_space && room.parent_id.is_empty(),
        server_name: parent_name.unwrap_or_default(),
        ..Default::default()
    };
    if chat.is_channel && chat.server_name.is_empty() && !room.bridge_id.is_empty() {
        let mut name = room.bridge_id.clone();
        if let Some(first) = name.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        chat.server_name = format!("{name} Server");
    }
    if let Some(ev_id) = ev_id {
        // The preview is rendered from the stored content; the read path is shared with messages.
        chat.preview = super::messages::preview_of(
            &ev_id,
            ev_sender.as_deref().unwrap_or_default(),
            ev_type.as_deref().unwrap_or_default(),
            ev_content.as_deref().unwrap_or("{}"),
            ev_edit.as_deref(),
            sender_name.as_deref().unwrap_or_default(),
        );
    }
    Ok(chat)
}

