//go:build !sqlcipher

package security

import (
	"errors"
)

// EncryptionAvailable reports whether this build can encrypt databases at rest.
const EncryptionAvailable = false

// errNoSQLCipher is returned by the encryption entry points of a build without SQLCipher.
var errNoSQLCipher = errors.New("security: database encryption requires the sqlcipher build tag")

// RegisterEncryptedDriver fails in a build without the sqlcipher tag: a key must never be accepted
// by a binary that would store the databases in plaintext. The engine checks EncryptionAvailable
// first and warns instead.
func RegisterEncryptedDriver(keyHex string) error {
	if _, err := parseKey(keyHex); err != nil {
		return err
	}
	return errNoSQLCipher
}

// ExportEncryptedCopy needs SQLCipher, so it is not available in a build without the sqlcipher tag.
func ExportEncryptedCopy(plainPath, encPath, keyHex string) error {
	return errNoSQLCipher
}

// VerifyEncrypted needs SQLCipher, so it is not available in a build without the sqlcipher tag.
func VerifyEncrypted(path string, key []byte) error {
	return errNoSQLCipher
}
