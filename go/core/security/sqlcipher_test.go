//go:build sqlcipher

package security

import (
	"bytes"
	"context"
	"database/sql"
	"encoding/hex"
	"os"
	"path/filepath"
	"testing"
)

const sqlitePlainHeader = "SQLite format 3\x00"

func randomKeyHex(t *testing.T) string {
	t.Helper()
	return hex.EncodeToString(randomBytes(t, keyBytes))
}

func mustExec(t *testing.T, db *sql.DB, statements ...string) {
	t.Helper()
	for _, stmt := range statements {
		if _, err := db.Exec(stmt); err != nil {
			t.Fatalf("%s: %v", stmt, err)
		}
	}
}

// createSampleDB fills a database through the process-wide driver name. Callers choose the name, so
// the test covers the driver that the core and the bridges actually use.
func createSampleDB(t *testing.T, driverName, path string) {
	t.Helper()
	db, err := sql.Open(driverName, path)
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	mustExec(t, db,
		"CREATE TABLE messages (id INTEGER PRIMARY KEY, body TEXT)",
		"INSERT INTO messages (body) VALUES ('one'), ('two'), ('three')",
	)
}

// assertEncryptedSample checks that path is an encrypted database with three messages: no plaintext
// header, unreadable without the key, unreadable with a wrong key, readable with the right key.
func assertEncryptedSample(t *testing.T, path, keyHex string) {
	t.Helper()
	ctx := context.Background()

	head, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if len(head) >= len(sqlitePlainHeader) && string(head[:len(sqlitePlainHeader)]) == sqlitePlainHeader {
		t.Fatal("database file starts with the plaintext SQLite header")
	}

	plain, err := openPlain(path, true)
	if err != nil {
		t.Fatal(err)
	}
	defer plain.Close()
	if _, err := tableCounts(ctx, plain); err == nil {
		t.Fatal("database opened without a key")
	}

	wrongKey := randomKeyHex(t)
	bad, err := openKeyed(path, wrongKey, true)
	if err != nil {
		t.Fatal(err)
	}
	defer bad.Close()
	if _, err := tableCounts(ctx, bad); err == nil {
		t.Fatal("database opened with a wrong key")
	}

	good, err := openKeyed(path, keyHex, true)
	if err != nil {
		t.Fatal(err)
	}
	defer good.Close()
	counts, err := tableCounts(ctx, good)
	if err != nil {
		t.Fatalf("open with right key: %v", err)
	}
	if counts["messages"] != 3 {
		t.Fatalf("messages rows = %d, want 3", counts["messages"])
	}
}

func TestRegisterEncryptedDriverKeysEveryDriver(t *testing.T) {
	keyHex := randomKeyHex(t)
	if err := RegisterEncryptedDriver(keyHex); err != nil {
		t.Fatalf("RegisterEncryptedDriver: %v", err)
	}

	for _, driverName := range []string{"sqlite3", "sqlite3-fk-wal"} {
		t.Run(driverName, func(t *testing.T) {
			path := filepath.Join(t.TempDir(), "chirper.db")
			createSampleDB(t, driverName, path)
			assertEncryptedSample(t, path, keyHex)
		})
	}
}

func TestRegisterEncryptedDriverKeepsBridgePragmas(t *testing.T) {
	if err := RegisterEncryptedDriver(randomKeyHex(t)); err != nil {
		t.Fatal(err)
	}
	// sqlite3-fk-wal sets journal_mode=WAL in its own ConnectHook. Chaining must keep that hook.
	db, err := sql.Open("sqlite3-fk-wal", filepath.Join(t.TempDir(), "bridge.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	var mode string
	if err := db.QueryRow("PRAGMA journal_mode").Scan(&mode); err != nil {
		t.Fatal(err)
	}
	if mode != "wal" {
		t.Fatalf("journal_mode = %q, want wal: the bridge hook did not run", mode)
	}
}

func TestRegisterEncryptedDriverReplacesKey(t *testing.T) {
	first, second := randomKeyHex(t), randomKeyHex(t)
	if err := RegisterEncryptedDriver(first); err != nil {
		t.Fatal(err)
	}
	if err := RegisterEncryptedDriver(second); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(t.TempDir(), "chirper.db")
	createSampleDB(t, "sqlite3", path)
	assertEncryptedSample(t, path, second)
}

func TestSQLCipherIsLinked(t *testing.T) {
	if err := requireSQLCipher(); err != nil {
		t.Fatal(err)
	}
}

// fileSnapshot returns the contents of each path, with nil for files that do not exist.
func fileSnapshot(t *testing.T, paths ...string) map[string][]byte {
	t.Helper()
	out := make(map[string][]byte, len(paths))
	for _, p := range paths {
		b, err := os.ReadFile(p)
		if err != nil && !os.IsNotExist(err) {
			t.Fatal(err)
		}
		out[p] = b
	}
	return out
}

func TestExportEncryptedCopy(t *testing.T) {
	dir := t.TempDir()
	src := filepath.Join(dir, "chirper.db")
	enc := filepath.Join(dir, "chirper.enc.db")
	keyHex := randomKeyHex(t)

	// Keep the writer open and skip checkpoints, so the rows live only in the -wal file. An export
	// that wrote through the source would then change the main file when it closes.
	writer, err := openPlain(src, false)
	if err != nil {
		t.Fatal(err)
	}
	defer writer.Close()
	mustExec(t, writer,
		"PRAGMA journal_mode = WAL",
		"PRAGMA wal_autocheckpoint = 0",
		"PRAGMA user_version = 7",
		"CREATE TABLE messages (id INTEGER PRIMARY KEY, body TEXT)",
		"INSERT INTO messages (body) VALUES ('one'), ('two'), ('three')",
		"CREATE TABLE rooms (id INTEGER PRIMARY KEY, name TEXT)",
		"INSERT INTO rooms (name) VALUES ('general'), ('family')",
	)
	before := fileSnapshot(t, src, src+"-wal")

	if err := ExportEncryptedCopy(src, enc, keyHex); err != nil {
		t.Fatalf("ExportEncryptedCopy: %v", err)
	}

	after := fileSnapshot(t, src, src+"-wal")
	for path, want := range before {
		if !bytes.Equal(after[path], want) {
			t.Fatalf("source file %s changed during export", filepath.Base(path))
		}
	}

	assertEncryptedSample(t, enc, keyHex)
	copyDB, err := openKeyed(enc, keyHex, true)
	if err != nil {
		t.Fatal(err)
	}
	defer copyDB.Close()
	counts, err := tableCounts(context.Background(), copyDB)
	if err != nil {
		t.Fatal(err)
	}
	if counts["rooms"] != 2 {
		t.Fatalf("rooms rows = %d, want 2", counts["rooms"])
	}
	version, err := userVersion(context.Background(), copyDB)
	if err != nil || version != 7 {
		t.Fatalf("user_version = %d, %v; want 7", version, err)
	}
}

func TestExportEncryptedCopyRefusesExistingDestination(t *testing.T) {
	dir := t.TempDir()
	src := filepath.Join(dir, "chirper.db")
	enc := filepath.Join(dir, "chirper.enc.db")
	createSampleDB(t, "sqlite3", src)
	writeFile(t, enc, []byte("keep me"))

	if err := ExportEncryptedCopy(src, enc, randomKeyHex(t)); err == nil {
		t.Fatal("ExportEncryptedCopy overwrote an existing destination")
	}
	if got, _ := os.ReadFile(enc); string(got) != "keep me" {
		t.Fatal("existing destination was modified")
	}
}

func TestExportEncryptedCopyMissingSourceLeavesNothing(t *testing.T) {
	dir := t.TempDir()
	enc := filepath.Join(dir, "chirper.enc.db")
	if err := ExportEncryptedCopy(filepath.Join(dir, "missing.db"), enc, randomKeyHex(t)); err == nil {
		t.Fatal("ExportEncryptedCopy succeeded for a missing source")
	}
	if _, err := os.Stat(enc); !os.IsNotExist(err) {
		t.Fatalf("destination created for a failed export: %v", err)
	}
}
