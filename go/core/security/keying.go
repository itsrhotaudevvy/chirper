//go:build sqlcipher

package security

import (
	"context"
	"database/sql"
	"database/sql/driver"
	"errors"
	"net/url"
	"path/filepath"
	"sync"

	"github.com/mattn/go-sqlite3"
	// Registers the "sqlite3-fk-wal" and "litestream" driver names that mautrix bridges use, so
	// RegisterEncryptedDriver sees them regardless of import order in the core.
	_ "go.mau.fi/util/dbutil/litestream"
)

var (
	registerMu sync.Mutex
	// baseHooks remembers each driver's ConnectHook from before we wrapped it, so that calling
	// RegisterEncryptedDriver again does not stack a second hook.
	baseHooks = map[*sqlite3.SQLiteDriver]func(*sqlite3.SQLiteConn) error{}
)

// EncryptionAvailable reports whether this build can encrypt databases at rest.
const EncryptionAvailable = true

// RegisterEncryptedDriver makes every SQLite connection opened through mattn/go-sqlite3 in this
// process use the given 32-byte raw key (hex). Connections are keyed as they open (see
// keying_cgo.go), before any statement runs. Call it once at startup, before opening any database.
// A later call replaces the key for connections opened after it.
func RegisterEncryptedDriver(keyHex string) error {
	key, err := parseKey(keyHex)
	if err != nil {
		return err
	}
	if err := requireSQLCipher(); err != nil {
		return err
	}

	registerMu.Lock()
	defer registerMu.Unlock()
	setDefaultKey(key)
	for _, name := range sql.Drivers() {
		drv, err := sqliteDriver(name)
		if err != nil {
			return err
		}
		if drv == nil {
			continue
		}
		base, seen := baseHooks[drv]
		if !seen {
			base = drv.ConnectHook
			baseHooks[drv] = base
		}
		drv.ConnectHook = chainHooks(memorySecurityHook, base)
	}
	return nil
}

// sqliteDriver returns the mattn driver registered under name, or nil when that driver is not a
// mattn driver. sql.Open does not connect, so this has no side effects on the database.
func sqliteDriver(name string) (*sqlite3.SQLiteDriver, error) {
	db, err := sql.Open(name, "")
	if err != nil {
		return nil, err
	}
	defer db.Close()
	drv, _ := db.Driver().(*sqlite3.SQLiteDriver)
	return drv, nil
}

// chainHooks runs first, then next (when set).
func chainHooks(first, next func(*sqlite3.SQLiteConn) error) func(*sqlite3.SQLiteConn) error {
	return func(conn *sqlite3.SQLiteConn) error {
		if err := first(conn); err != nil {
			return err
		}
		if next == nil {
			return nil
		}
		return next(conn)
	}
}

// memorySecurityHook runs on every keyed connection after it is open. It keeps freed page contents
// out of memory.
func memorySecurityHook(conn *sqlite3.SQLiteConn) error {
	_, err := conn.Exec("PRAGMA cipher_memory_security = ON", nil)
	return err
}

// requireSQLCipher fails closed when the binary was linked against a plain SQLite. On plain SQLite
// the key is silently ignored and the databases would stay unencrypted.
func requireSQLCipher() error {
	db := sql.OpenDB(connector{drv: &sqlite3.SQLiteDriver{}, dsn: ":memory:"})
	defer db.Close()
	var version string
	if err := db.QueryRow("PRAGMA cipher_version").Scan(&version); err != nil || version == "" {
		return errors.New("security: sqlite3 library is not SQLCipher (build with -tags libsqlite3,sqlcipher)")
	}
	return nil
}

// connector opens connections through one driver value, in one open mode (see openWith). Using a
// fresh driver instead of the process-wide one lets a connection skip the default key (plaintext
// reads) or use a different key (verification).
type connector struct {
	drv   *sqlite3.SQLiteDriver
	dsn   string
	plain bool
	key   []byte
}

func (c connector) Connect(context.Context) (driver.Conn, error) {
	return openWith(c.drv, c.dsn, c.plain, c.key)
}

func (c connector) Driver() driver.Driver {
	return c.drv
}

// openPlain opens an unkeyed database. readOnly opens it with mode=ro, which never writes to the
// file or its -wal/-shm and does not checkpoint on close.
func openPlain(path string, readOnly bool) (*sql.DB, error) {
	dsn, err := fileURI(path, readOnly)
	if err != nil {
		return nil, err
	}
	drv := &sqlite3.SQLiteDriver{ConnectHook: memorySecurityHook}
	return sql.OpenDB(connector{drv: drv, dsn: dsn, plain: true}), nil
}

// openKeyed opens a database with the given raw key (hex), using a dedicated driver value.
func openKeyed(path, keyHex string, readOnly bool) (*sql.DB, error) {
	key, err := parseKey(keyHex)
	if err != nil {
		return nil, err
	}
	dsn, err := fileURI(path, readOnly)
	if err != nil {
		return nil, err
	}
	drv := &sqlite3.SQLiteDriver{ConnectHook: memorySecurityHook}
	return sql.OpenDB(connector{drv: drv, dsn: dsn, key: key}), nil
}

// fileURI builds a file: URI so that mode=ro is honoured and paths with '?' or '#' are safe.
func fileURI(path string, readOnly bool) (string, error) {
	abs, err := filepath.Abs(path)
	if err != nil {
		return "", err
	}
	u := url.URL{Scheme: "file", Path: abs}
	if readOnly {
		u.RawQuery = "mode=ro"
	}
	return u.String(), nil
}
