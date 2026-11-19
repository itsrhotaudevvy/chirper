//! The local timeline database: one SQLCipher file (`chirper.db`) per install.
//!
//! The schema is the one the Go core wrote (v1..v5), so an existing install keeps its history, plus
//! v6 which adds the remote-id mappings and the login table that the unified bridge runtime needs.
//! There is no per-bridge database any more.
//!
//! Message content is stored as versioned, Matrix-free JSON (see [`crate::engine::content`]);
//! rows written by the Go core are still read through a compatibility parser.

mod events;
mod media;
mod messages;
mod rooms;

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, params};

pub use events::{Event, Reaction, Redaction};
pub use media::{Media, media_dir};
pub use rooms::{Member, Profile, Room, RoomFields};

use crate::model::{Chat, ChatFilter, Message};

/// A store failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not found")]
    NotFound,
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("{0}")]
    Other(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// The latest schema version this build understands.
pub const SCHEMA_VERSION: i64 = 6;

/// An open timeline database. Safe for concurrent use: one connection, serialised in process.
pub struct Store {
    conn: Mutex<Connection>,
}

/// A login row: the accounts that exist without a live connection.
#[derive(Clone, Debug, Default)]
pub struct Login {
    pub network: String,
    pub login_id: String,
    pub name: String,
    pub avatar_url: String,
    pub remote_id: String,
    pub added_ts: i64,
}

impl Store {
    /// Open (and migrate) the database at `path`. `key` is the raw 32-byte SQLCipher key, or `None`
    /// for a plaintext database (development and tests only).
    pub fn open(path: &Path, key: Option<&[u8; 32]>) -> Result<Self> {
        if key.is_some() && !cfg!(feature = "sqlcipher") {
            // Refusing is the safe failure: writing plaintext while the shell believes the file is
            // encrypted would be undetectable afterwards.
            return Err(Error::Other(
                "this build has no SQLCipher; open the database without a key".to_owned(),
            ));
        }
        let conn = Connection::open(path)?;
        if let Some(key) = key {
            // Raw key: SQLCipher skips PBKDF2, and the key never appears as a passphrase.
            conn.pragma_update(None, "key", format!("x'{}'", hex::encode(key)))?;
        }
        // Per-connection settings. WAL, overwrite freed pages, keep sort spills in memory.
        for pragma in [
            "PRAGMA foreign_keys = ON",
            "PRAGMA journal_mode = WAL",
            "PRAGMA synchronous = NORMAL",
            "PRAGMA busy_timeout = 5000",
            "PRAGMA temp_store = MEMORY",
            "PRAGMA cache_size = -16000",
            "PRAGMA secure_delete = ON",
        ] {
            conn.execute_batch(pragma)?;
        }
        if key.is_some() {
            conn.execute_batch("PRAGMA cipher_memory_security = ON")?;
        }
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    /// An in-memory database, for tests.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys = ON")?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().expect("store connection")
    }

    /// Run `f` in an immediate transaction. Every write takes the write lock up front, which is what
    /// the Go core got from `_txlock=immediate`.
    fn write<T>(&self, f: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T>) -> Result<T> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    /// The current schema version of this file.
    pub fn version(&self) -> Result<i64> {
        let conn = self.conn();
        read_version(&conn)
    }

    /// Close the database. Later calls on a clone fail.
    pub fn close(self) {}
}

/// Read the highest applied version, from either the Go version table or the Rust one.
fn read_version(conn: &Connection) -> Result<i64> {
    let mut version: i64 = 0;
    if table_exists(conn, "chirper_version")? {
        version = version.max(
            conn.query_row(
                "SELECT COALESCE(MAX(version), 0) FROM chirper_version",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0),
        );
    }
    if table_exists(conn, "chirper_migrations")? {
        version = version.max(
            conn.query_row(
                "SELECT COALESCE(MAX(version), 0) FROM chirper_migrations",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0),
        );
    }
    Ok(version)
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            params![name],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

impl Store {
    fn migrate(&self) -> Result<()> {
        let mut conn = self.conn();
        let from = read_version(&conn)?;
        if from >= SCHEMA_VERSION {
            return Ok(());
        }
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS chirper_migrations (
                version INTEGER PRIMARY KEY,
                applied_ts INTEGER NOT NULL DEFAULT 0,
                name TEXT NOT NULL DEFAULT ''
            )",
        )?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for (version, name, sql) in MIGRATIONS {
            if *version <= from.max(0) {
                continue;
            }
            if *version == 4 {
                // Older installs lack the column; fresh ones got it in v1.
                ensure_column(&tx, "room", "parent_id", "TEXT NOT NULL DEFAULT ''")?;
            }
            tx.execute_batch(sql)
                .map_err(|e| Error::Other(format!("migration {version} ({name}) failed: {e}")))?;
            tx.execute(
                "INSERT OR REPLACE INTO chirper_migrations (version, applied_ts, name)
                 VALUES (?1, ?2, ?3)",
                params![version, now_ms(), name],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

/// Add a column when the table does not have it yet.
fn ensure_column(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    let mut stmt = tx.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if names.iter().any(|name| name == column) {
        return Ok(());
    }
    tx.execute_batch(&format!(
        "ALTER TABLE {table} ADD COLUMN {column} {definition}"
    ))?;
    Ok(())
}

/// Unix milliseconds.
pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A LIKE pattern that matches `query` as a substring, escaping wildcards.
pub fn like_pattern(query: &str) -> String {
    let mut out = String::with_capacity(query.len() + 2);
    out.push('%');
    for ch in query.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('%');
    out
}

/// Every migration, in order. Steps 1..=5 are the Go schema, byte for byte, so an install written by
/// the Go core opens with its history intact.
const MIGRATIONS: &[(i64, &str, &str)] = &[
    (1, "initial schema", SCHEMA_V1),
    (2, "chat list and reaction indices", SCHEMA_V2),
    (3, "event media links", SCHEMA_V3),
    (4, "room parent_id", SCHEMA_V4),
    (5, "chat order by last message", SCHEMA_V5),
    (6, "remote ids, logins and backfill", SCHEMA_V6),
];

const SCHEMA_V1: &str = r#"
CREATE TABLE room (
	room_id          TEXT PRIMARY KEY,
	bridge_id        TEXT NOT NULL,
	login_id         TEXT NOT NULL DEFAULT '',
	portal_id        TEXT NOT NULL DEFAULT '',
	name             TEXT NOT NULL DEFAULT '',
	topic            TEXT NOT NULL DEFAULT '',
	avatar_url       TEXT NOT NULL DEFAULT '',
	is_dm            BOOLEAN NOT NULL DEFAULT false,
	is_space         BOOLEAN NOT NULL DEFAULT false,
	dm_user_id       TEXT NOT NULL DEFAULT '',
	created_ts       BIGINT NOT NULL DEFAULT 0,
	last_activity_ts BIGINT NOT NULL DEFAULT 0,
	last_event_id    TEXT NOT NULL DEFAULT '',
	unread_count     INTEGER NOT NULL DEFAULT 0,
	mention_count    INTEGER NOT NULL DEFAULT 0,
	read_up_to_order BIGINT NOT NULL DEFAULT 0,
	pinned           BOOLEAN NOT NULL DEFAULT false,
	archived         BOOLEAN NOT NULL DEFAULT false,
	muted_until      BIGINT NOT NULL DEFAULT 0,
	tags_json        TEXT NOT NULL DEFAULT '{}',
	deleted          BOOLEAN NOT NULL DEFAULT false,
	parent_id        TEXT NOT NULL DEFAULT ''
);
CREATE INDEX room_bridge_idx ON room(bridge_id);
CREATE INDEX room_parent_idx ON room(parent_id);

CREATE TABLE member (
	room_id     TEXT NOT NULL,
	user_id     TEXT NOT NULL,
	displayname TEXT NOT NULL DEFAULT '',
	avatar_url  TEXT NOT NULL DEFAULT '',
	membership  TEXT NOT NULL,
	PRIMARY KEY (room_id, user_id)
);
CREATE INDEX member_user_idx ON member(user_id);

CREATE TABLE profile (
	user_id     TEXT PRIMARY KEY,
	displayname TEXT NOT NULL DEFAULT '',
	avatar_url  TEXT NOT NULL DEFAULT '',
	extra_json  TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE event (
	event_id          TEXT PRIMARY KEY,
	room_id           TEXT NOT NULL,
	ord               INTEGER NOT NULL,
	sender            TEXT NOT NULL,
	type              TEXT NOT NULL,
	state_key         TEXT,
	content_json      TEXT NOT NULL,
	ts                INTEGER NOT NULL,
	txn_id            TEXT,
	reply_to          TEXT NOT NULL DEFAULT '',
	edit_content_json TEXT,
	edited_ts         INTEGER NOT NULL DEFAULT 0,
	redacted          BOOLEAN NOT NULL DEFAULT false,
	status            TEXT NOT NULL DEFAULT '',
	error             TEXT NOT NULL DEFAULT '',
	part_index        INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX event_room_ord_idx ON event(room_id, ord);
CREATE UNIQUE INDEX event_room_txn_idx ON event(room_id, txn_id) WHERE txn_id IS NOT NULL;

CREATE TABLE reaction (
	event_id        TEXT PRIMARY KEY,
	room_id         TEXT NOT NULL,
	target_event_id TEXT NOT NULL,
	sender          TEXT NOT NULL,
	key             TEXT NOT NULL,
	ts              INTEGER NOT NULL
);
CREATE INDEX reaction_target_idx ON reaction(target_event_id);

CREATE TABLE media (
	media_id           TEXT PRIMARY KEY,
	mime               TEXT NOT NULL DEFAULT '',
	size               INTEGER NOT NULL DEFAULT 0,
	file_name          TEXT NOT NULL DEFAULT '',
	path               TEXT NOT NULL,
	encrypted_key_json TEXT,
	created_ts         INTEGER NOT NULL
);
"#;

const SCHEMA_V2: &str = r#"
CREATE INDEX room_list_idx ON room(deleted, archived, pinned DESC, last_activity_ts DESC, room_id);
DROP INDEX reaction_target_idx;
CREATE INDEX reaction_target_idx ON reaction(target_event_id, sender, key);
"#;

const SCHEMA_V3: &str = r#"
CREATE TABLE event_media (
	event_id TEXT NOT NULL REFERENCES event(event_id) ON DELETE CASCADE,
	room_id  TEXT NOT NULL,
	media_id TEXT NOT NULL,
	PRIMARY KEY (event_id, media_id)
);
CREATE INDEX event_media_media_idx ON event_media(media_id);
CREATE INDEX event_media_room_idx ON event_media(room_id);
"#;

const SCHEMA_V4: &str = r#"
CREATE INDEX IF NOT EXISTS room_parent_idx ON room(parent_id);
"#;

const SCHEMA_V5: &str = r#"
UPDATE room SET
	last_activity_ts = COALESCE((
		SELECT MAX(ts) FROM event
		WHERE event.room_id = room.room_id AND event.state_key IS NULL
			AND event.type IN ('m.room.message', 'm.sticker')), created_ts),
	last_event_id = COALESCE((
		SELECT event_id FROM event
		WHERE event.room_id = room.room_id AND event.state_key IS NULL
			AND event.type IN ('m.room.message', 'm.sticker')
		ORDER BY ts DESC, ord DESC LIMIT 1), '');
"#;

/// v6 keeps the remote identity of every row, so a driver can address what the store holds without
/// a second database, and records the accounts that exist without a live connection.
const SCHEMA_V6: &str = r#"
ALTER TABLE room ADD COLUMN remote_id TEXT NOT NULL DEFAULT '';
ALTER TABLE room ADD COLUMN parent_remote_id TEXT NOT NULL DEFAULT '';
ALTER TABLE event ADD COLUMN remote_id TEXT NOT NULL DEFAULT '';
CREATE INDEX event_remote_idx ON event(room_id, remote_id);
CREATE TABLE login (
	network    TEXT NOT NULL,
	login_id   TEXT NOT NULL,
	name       TEXT NOT NULL DEFAULT '',
	avatar_url TEXT NOT NULL DEFAULT '',
	remote_id  TEXT NOT NULL DEFAULT '',
	added_ts   INTEGER NOT NULL DEFAULT 0,
	PRIMARY KEY (network, login_id)
);
CREATE TABLE backfill (
	room_id TEXT PRIMARY KEY,
	cursor  TEXT NOT NULL DEFAULT '',
	done    BOOLEAN NOT NULL DEFAULT false
);
"#;

/// Re-typed for the public API: the store answers with the wire model.
pub type ChatRow = Chat;
pub type MessageRow = Message;
pub type ChatFilterRow = ChatFilter;
