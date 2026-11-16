//go:build sqlcipher

package security

import (
	"encoding/hex"
	"path/filepath"
	"testing"
)

func TestVerifyEncryptedAcceptsOnlyAKeyedDatabase(t *testing.T) {
	dir := t.TempDir()
	key := randomBytes(t, keyBytes)
	keyed := filepath.Join(dir, "keyed.db")
	plain := filepath.Join(dir, "plain.db")

	plainDB, err := openPlain(plain, false)
	if err != nil {
		t.Fatal(err)
	}
	mustExec(t, plainDB, "CREATE TABLE t (x)", "INSERT INTO t VALUES (1)")
	if err := plainDB.Close(); err != nil {
		t.Fatal(err)
	}
	if err := VerifyEncrypted(plain, key); err == nil {
		t.Fatal("VerifyEncrypted accepted a plaintext database")
	}

	db, err := openKeyed(keyed, hex.EncodeToString(key), false)
	if err != nil {
		t.Fatal(err)
	}
	mustExec(t, db, "CREATE TABLE t (x)", "INSERT INTO t VALUES (1)")
	if err := db.Close(); err != nil {
		t.Fatal(err)
	}
	if err := VerifyEncrypted(keyed, key); err != nil {
		t.Fatalf("VerifyEncrypted rejected an encrypted database: %v", err)
	}
	if err := VerifyEncrypted(keyed, randomBytes(t, keyBytes)); err == nil {
		t.Fatal("VerifyEncrypted accepted a wrong key")
	}
}
