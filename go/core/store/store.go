// Package store is the local timeline database of Chirper (chirper.db).
package store

import (
	"context"
	"database/sql"
	"database/sql/driver"
	"errors"
	"fmt"
	"strings"

	"github.com/mattn/go-sqlite3"
	"go.mau.fi/util/dbutil"
)

// driverName is the SQLite driver the store opens. Its connect hook applies connectPragmas to every
// connection, because these settings are per connection and must not depend on the first one.
const driverName = "sqlite3-chirper"

// connectPragmas are applied to each new connection.
var connectPragmas = []string{
	"PRAGMA foreign_keys = ON",
	"PRAGMA journal_mode = WAL",
	"PRAGMA synchronous = NORMAL",
	"PRAGMA busy_timeout = 5000",
	// Temporary tables and sort spills stay in memory, so message text never lands in a temp file.
	"PRAGMA temp_store = MEMORY",
	// 16 MiB page cache per connection (negative values are KiB).
	"PRAGMA cache_size = -16000",
	// Freed pages are overwritten, so deleted message content is not left behind in the file.
	"PRAGMA secure_delete = ON",
}

func init() {
	sql.Register(driverName, &sqlite3.SQLiteDriver{
		ConnectHook: func(conn *sqlite3.SQLiteConn) error {
			for _, pragma := range connectPragmas {
				if _, err := conn.Exec(pragma, []driver.Value{}); err != nil {
					return fmt.Errorf("failed to apply %q: %w", pragma, err)
				}
			}
			return nil
		},
	})
}

// ErrNotFound is returned by lookups for rows that do not exist.
var ErrNotFound = errors.New("not found")

// Store is the SQLite-backed timeline store. It is safe for concurrent use: SQLite is
// accessed through a single connection.
type Store struct {
	db *dbutil.Database
}

const schemaV1 = `
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
`

// schemaV2 adds the indices of the hot queries: the chat list is filtered and sorted by these
// columns, and reactions are looked up by target and sender.
const schemaV2 = `
CREATE INDEX room_list_idx ON room(deleted, archived, pinned DESC, last_activity_ts DESC, room_id);
DROP INDEX reaction_target_idx;
CREATE INDEX reaction_target_idx ON reaction(target_event_id, sender, key);
`

// upgradeSteps returns the migration chain up to schema version last. Open runs the whole chain;
// tests use shorter chains to build databases as an earlier release wrote them.
func upgradeSteps(last int) dbutil.UpgradeTable {
	steps := dbutil.BuildUpgradeTable().
		WithRaw(0, 1, 1, "initial schema", dbutil.TxnModeOn, func(ctx context.Context, db *dbutil.Database) error {
			_, err := db.Exec(ctx, schemaV1)
			return err
		}).
		WithRaw(1, 2, 2, "chat list and reaction indices", dbutil.TxnModeOn, func(ctx context.Context, db *dbutil.Database) error {
			_, err := db.Exec(ctx, schemaV2)
			return err
		})
	if last >= 3 {
		steps = steps.WithRaw(2, 3, 3, "event media links", dbutil.TxnModeOn, migrateMediaLinks)
	}
	if last >= 4 {
		steps = steps.WithRaw(3, 4, 4, "add room parent_id column", dbutil.TxnModeOn, migrateAddRoomParentID)
	}
	return steps.Finish()
}

func migrateAddRoomParentID(ctx context.Context, db *dbutil.Database) error {
	rows, err := db.Query(ctx, `PRAGMA table_info(room)`)
	if err != nil {
		return err
	}
	defer rows.Close()
	hasParent := false
	for rows.Next() {
		var (
			cid     int
			name    string
			ctype   string
			notnull int
			dfltVal sql.NullString
			pk      int
		)
		if err := rows.Scan(&cid, &name, &ctype, &notnull, &dfltVal, &pk); err == nil && name == "parent_id" {
			hasParent = true
			break
		}
	}
	if !hasParent {
		if _, err := db.Exec(ctx, `ALTER TABLE room ADD COLUMN parent_id TEXT NOT NULL DEFAULT ''`); err != nil {
			return err
		}
	}
	_, err = db.Exec(ctx, `CREATE INDEX IF NOT EXISTS room_parent_idx ON room(parent_id)`)
	return err
}

// Open opens (and migrates) the store at path. The file is created if it does not exist.
func Open(ctx context.Context, path string) (*Store, error) {
	return openWithUpgrades(ctx, path, upgradeSteps(4))
}

// openWithUpgrades opens the store at path and migrates it with steps.
func openWithUpgrades(ctx context.Context, path string, steps dbutil.UpgradeTable) (*Store, error) {
	// One connection: SQLite allows a single writer, and a single connection serialises readers
	// and writers in-process so that no statement waits on the busy timeout.
	db, err := dbutil.NewFromConfig("chirper", dbutil.Config{
		PoolConfig: dbutil.PoolConfig{
			Type:         driverName,
			URI:          path + "?_txlock=immediate",
			MaxOpenConns: 1,
			MaxIdleConns: 1,
		},
	}, dbutil.NoopLogger)
	if err != nil {
		return nil, fmt.Errorf("failed to open store database: %w", err)
	}
	db.VersionTable = "chirper_version"
	db.UpgradeTable = steps
	if err = db.Upgrade(ctx); err != nil {
		_ = db.Close()
		return nil, fmt.Errorf("failed to upgrade store database: %w", err)
	}
	return &Store{db: db}, nil
}

// Close closes the database.
func (s *Store) Close() error {
	return s.db.Close()
}

// rowScanner is satisfied by both *sql.Row and dbutil.Rows.
type rowScanner interface {
	Scan(dest ...any) error
}

// queryAll runs a query and scans every row with scan. The cursor is closed before returning,
// so it is safe to issue further queries afterwards (the store uses a single connection).
func queryAll[T any](ctx context.Context, s *Store, scan func(rowScanner) (T, error), query string, args ...any) ([]T, error) {
	rows, err := s.db.Query(ctx, query, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []T
	for rows.Next() {
		item, err := scan(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, item)
	}
	return out, rows.Err()
}

// queryOne runs a query expected to return one row, mapping sql.ErrNoRows to ErrNotFound.
func queryOne[T any](ctx context.Context, s *Store, scan func(rowScanner) (T, error), query string, args ...any) (T, error) {
	var zero T
	item, err := scan(s.db.QueryRow(ctx, query, args...))
	if errors.Is(err, sql.ErrNoRows) {
		return zero, ErrNotFound
	} else if err != nil {
		return zero, err
	}
	return item, nil
}

// likePattern builds a LIKE pattern that matches query as a substring, escaping wildcards.
func likePattern(query string) string {
	r := strings.NewReplacer(`\`, `\\`, `%`, `\%`, `_`, `\_`)
	return "%" + r.Replace(query) + "%"
}
