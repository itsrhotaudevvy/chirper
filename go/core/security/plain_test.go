//go:build !sqlcipher

package security

import (
	"strings"
	"testing"
)

func TestPlainBuildRegisterFailsClosed(t *testing.T) {
	if EncryptionAvailable {
		t.Fatal("EncryptionAvailable is true without the sqlcipher tag")
	}
	if err := RegisterEncryptedDriver(strings.Repeat("ab", keyBytes)); err == nil {
		t.Fatal("RegisterEncryptedDriver accepted a key without SQLCipher")
	}
}

func TestPlainBuildExportIsUnavailable(t *testing.T) {
	dir := t.TempDir()
	err := ExportEncryptedCopy(dir+"/a.db", dir+"/b.db", strings.Repeat("ab", keyBytes))
	if err == nil {
		t.Fatal("ExportEncryptedCopy succeeded without SQLCipher")
	}
}
