//go:build sqlcipher

package security

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"strings"
)

// ExportEncryptedCopy writes an encrypted copy of the plaintext database at plainPath to encPath.
// encPath must not exist. The source is opened read-only and is never modified or deleted, so it
// stays byte-identical, -wal included. If the export or the verification fails, the new file is
// removed and the error is returned.
//
// Verification opens the copy with keyHex and checks that it does not open without the key. It
// also checks that every user table has the same row count as the source, and that user_version
// matches.
func ExportEncryptedCopy(plainPath, encPath, keyHex string) error {
	if _, err := parseKey(keyHex); err != nil {
		return err
	}
	if _, err := os.Stat(plainPath); err != nil {
		return fmt.Errorf("security: source database: %w", err)
	}
	if _, err := os.Stat(encPath); err == nil {
		return fmt.Errorf("security: destination already exists: %s", encPath)
	} else if !errors.Is(err, fs.ErrNotExist) {
		return err
	}

	if err := exportCopy(plainPath, encPath, keyHex); err != nil {
		removeCopy(encPath)
		return err
	}
	if err := verifyCopy(plainPath, encPath, keyHex); err != nil {
		removeCopy(encPath)
		return err
	}
	return nil
}

// exportCopy runs the migration on one connection. That connection is the new keyed database
// (main), and the source is attached read-only as "plain". sqlcipher_export(target, source) copies
// the schema and data, and user_version is copied explicitly.
func exportCopy(plainPath, encPath, keyHex string) error {
	ctx := context.Background()
	dst, err := openKeyed(encPath, keyHex, false)
	if err != nil {
		return err
	}
	defer dst.Close()
	dst.SetMaxOpenConns(1) // ATTACH is per connection, so every statement must use this one.

	conn, err := dst.Conn(ctx)
	if err != nil {
		return err
	}
	defer conn.Close()

	src, err := fileURI(plainPath, true)
	if err != nil {
		return err
	}
	// KEY '' attaches the source unencrypted. Without it, SQLCipher keys the attached file with the
	// main database's key, and the plaintext source fails to open.
	if _, err := conn.ExecContext(ctx, "ATTACH DATABASE ? AS plain KEY ''", src); err != nil {
		return fmt.Errorf("security: attach source: %w", err)
	}
	if _, err := conn.ExecContext(ctx, "SELECT sqlcipher_export('main', 'plain')"); err != nil {
		return fmt.Errorf("security: export: %w", err)
	}
	var userVersion int64
	if err := conn.QueryRowContext(ctx, "PRAGMA plain.user_version").Scan(&userVersion); err != nil {
		return err
	}
	if _, err := conn.ExecContext(ctx, fmt.Sprintf("PRAGMA main.user_version = %d", userVersion)); err != nil {
		return err
	}
	_, err = conn.ExecContext(ctx, "DETACH DATABASE plain")
	return err
}

func verifyCopy(plainPath, encPath, keyHex string) error {
	ctx := context.Background()
	src, err := openPlain(plainPath, true)
	if err != nil {
		return err
	}
	defer src.Close()
	dst, err := openKeyed(encPath, keyHex, true)
	if err != nil {
		return err
	}
	defer dst.Close()

	want, err := tableCounts(ctx, src)
	if err != nil {
		return fmt.Errorf("security: count source rows: %w", err)
	}
	got, err := tableCounts(ctx, dst)
	if err != nil {
		return fmt.Errorf("security: open encrypted copy with key: %w", err)
	}
	if err := sameCounts(want, got); err != nil {
		return err
	}
	if err := mustNotOpenUnkeyed(ctx, encPath); err != nil {
		return err
	}
	return sameUserVersion(ctx, src, dst)
}

// tableCounts maps each user table to its row count.
func tableCounts(ctx context.Context, db *sql.DB) (map[string]int64, error) {
	names, err := userTables(ctx, db)
	if err != nil {
		return nil, err
	}
	counts := make(map[string]int64, len(names))
	for _, name := range names {
		var n int64
		query := fmt.Sprintf(`SELECT COUNT(*) FROM "%s"`, strings.ReplaceAll(name, `"`, `""`))
		if err := db.QueryRowContext(ctx, query).Scan(&n); err != nil {
			return nil, err
		}
		counts[name] = n
	}
	return counts, nil
}

func userTables(ctx context.Context, db *sql.DB) ([]string, error) {
	rows, err := db.QueryContext(ctx,
		`SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite\_%' ESCAPE '\'`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var names []string
	for rows.Next() {
		var name string
		if err := rows.Scan(&name); err != nil {
			return nil, err
		}
		names = append(names, name)
	}
	return names, rows.Err()
}

func sameCounts(want, got map[string]int64) error {
	if len(want) != len(got) {
		return fmt.Errorf("security: copy has %d tables, source has %d", len(got), len(want))
	}
	for table, n := range want {
		if got[table] != n {
			return fmt.Errorf("security: copy row count mismatch in table %q", table)
		}
	}
	return nil
}

// mustNotOpenUnkeyed fails if the copy can be read without the key, which would mean the export
// silently wrote plaintext.
func mustNotOpenUnkeyed(ctx context.Context, encPath string) error {
	db, err := openPlain(encPath, true)
	if err != nil {
		return err
	}
	defer db.Close()
	var n int64
	if err := db.QueryRowContext(ctx, "SELECT COUNT(*) FROM sqlite_master").Scan(&n); err == nil {
		return errors.New("security: encrypted copy is readable without the key")
	}
	return nil
}

func sameUserVersion(ctx context.Context, src, dst *sql.DB) error {
	want, err := userVersion(ctx, src)
	if err != nil {
		return err
	}
	got, err := userVersion(ctx, dst)
	if err != nil {
		return err
	}
	if want != got {
		return errors.New("security: copy user_version mismatch")
	}
	return nil
}

func userVersion(ctx context.Context, db *sql.DB) (int64, error) {
	var v int64
	err := db.QueryRowContext(ctx, "PRAGMA user_version").Scan(&v)
	return v, err
}

// removeCopy deletes a partially written copy. It is only called for encPath, which did not exist
// before the export, so the source and any pre-existing files are never touched.
func removeCopy(encPath string) {
	for _, suffix := range []string{"", "-journal", "-wal", "-shm"} {
		_ = os.Remove(encPath + suffix)
	}
}
