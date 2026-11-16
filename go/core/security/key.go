package security

import (
	"encoding/hex"
	"errors"
)

// keyBytes is the length of a raw SQLCipher key (AES-256). Keys are passed as raw bytes, so
// SQLCipher's PBKDF2 passphrase derivation is skipped.
const keyBytes = 32

// parseKey validates a hex-encoded raw key and returns its bytes. The error deliberately does not
// quote the input, so key material never reaches logs.
func parseKey(keyHex string) ([]byte, error) {
	key, err := hex.DecodeString(keyHex)
	if err != nil {
		return nil, errors.New("security: database key is not valid hex")
	}
	if len(key) != keyBytes {
		return nil, errors.New("security: database key must be 32 bytes")
	}
	return key, nil
}
