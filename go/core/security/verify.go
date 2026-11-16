//go:build sqlcipher

package security

import (
	"bytes"
	"context"
	"database/sql"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"

	"github.com/mattn/go-sqlite3"
)

// plaintextHeader starts every plaintext SQLite 3 database file.
var plaintextHeader = []byte("SQLite format 3\x00")

// VerifyEncrypted returns an error unless the database file at path is encrypted with key. The file
// must not start with the plaintext SQLite header, must not open without a key or with a different
// key, and must open with key. The file is only read.
func VerifyEncrypted(path string, key []byte) error {
	if len(key) != keyBytes {
		return errors.New("security: database key must be 32 bytes")
	}
	name := filepath.Base(path)
	head, err := readHead(path, len(plaintextHeader))
	if err != nil {
		return fmt.Errorf("security: read %s: %w", name, err)
	}
	if bytes.Equal(head, plaintextHeader) {
		return fmt.Errorf("security: %s starts with the plaintext SQLite header", name)
	}

	wrongKey := bytes.Clone(key)
	wrongKey[0] ^= 0xff
	if err := schemaReadable(path, nil); err == nil {
		return fmt.Errorf("security: %s opens without a key", name)
	}
	if err := schemaReadable(path, wrongKey); err == nil {
		return fmt.Errorf("security: %s opens with a wrong key", name)
	}
	if err := schemaReadable(path, key); err != nil {
		return fmt.Errorf("security: %s does not open with the key: %w", name, err)
	}
	return nil
}

// readHead returns the first n bytes of path, or fewer if the file is shorter.
func readHead(path string, n int) ([]byte, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	head := make([]byte, n)
	read, err := io.ReadFull(f, head)
	if errors.Is(err, io.ErrUnexpectedEOF) || errors.Is(err, io.EOF) {
		return head[:read], nil
	}
	return head, err
}

// schemaReadable reads the schema of the database at path, read-only. A nil key opens it unkeyed.
func schemaReadable(path string, key []byte) error {
	dsn, err := fileURI(path, true)
	if err != nil {
		return err
	}
	drv := &sqlite3.SQLiteDriver{ConnectHook: memorySecurityHook}
	db := sql.OpenDB(connector{drv: drv, dsn: dsn, plain: key == nil, key: key})
	defer db.Close()
	var n int64
	return db.QueryRowContext(context.Background(), "SELECT COUNT(*) FROM sqlite_master").Scan(&n)
}
