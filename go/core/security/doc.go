// Package security provides at-rest encryption for Chirper's local data.
//
// Databases. Every SQLite file the core opens (chirper.db and each bridge-*.db) goes through
// github.com/mattn/go-sqlite3 linked against SQLCipher 4. RegisterEncryptedDriver sets the process
// key. A sqlite3_auto_extension keys each connection inside sqlite3_open_v2 (keying_cgo.go), before
// mattn runs its own PRAGMAs. Keying in a connect hook is too late for an existing file, because
// those PRAGMAs read the schema. A connect hook then sets `PRAGMA cipher_memory_security = ON`.
//
// Why the hook is installed on several drivers: go.mau.fi/util/dbutil does not open databases with
// a single driver name. Config type "sqlite3" maps to the driver mattn registers as "sqlite3", but
// the "sqlite3-fk-wal" and "litestream" types (which all mautrix bridges use) are separate
// sql.Register calls in dbutil/litestream, each with its own *sqlite3.SQLiteDriver and ConnectHook.
// A hook on only the "sqlite3" driver would leave those databases in plaintext. So
// RegisterEncryptedDriver walks sql.Drivers(), and for every *sqlite3.SQLiteDriver it prepends the
// keying hook to the hook that driver already had. Consequences: call it once, after package init
// and before any database is opened. Drivers registered afterwards are not covered.
//
// Migration. ExportEncryptedCopy turns a plaintext database into a verified encrypted copy at a new
// path (ATTACH ... KEY + sqlcipher_export). It never modifies or deletes the source. Removing the
// plaintext original is a separate, explicit step owned by the caller.
//
// Vault. EncryptFile, OpenReader and DecryptToFile implement chunked AES-256-GCM for media files at
// rest (see vault.go). They do not depend on SQLCipher.
//
// Build tags. The `sqlcipher` tag enables the encrypted paths. It must be combined with the
// `libsqlite3` tag and CGO flags that point at native/out/<target>, see scripts/build-sqlcipher.sh.
// Without `sqlcipher`, RegisterEncryptedDriver logs a warning and does nothing, and
// ExportEncryptedCopy returns an error. Both behaviours keep the desktop tests runnable without
// SQLCipher. An unencrypted build must never be shipped.
package security
