package security

import (
	"strings"
	"testing"
)

func TestParseKey(t *testing.T) {
	valid := strings.Repeat("ab", keyBytes)
	key, err := parseKey(valid)
	if err != nil || len(key) != keyBytes {
		t.Fatalf("valid key rejected: %v", err)
	}

	for name, input := range map[string]string{
		"not hex":   strings.Repeat("zz", keyBytes),
		"too short": strings.Repeat("ab", keyBytes-1),
		"too long":  strings.Repeat("ab", keyBytes+1),
		"empty":     "",
	} {
		t.Run(name, func(t *testing.T) {
			_, err := parseKey(input)
			if err == nil {
				t.Fatal("invalid key accepted")
			}
			if input != "" && strings.Contains(err.Error(), input) {
				t.Fatal("error message echoes key material")
			}
		})
	}
}

func TestRegisterEncryptedDriverRejectsInvalidKey(t *testing.T) {
	if err := RegisterEncryptedDriver("not-a-key"); err == nil {
		t.Fatal("RegisterEncryptedDriver accepted an invalid key")
	}
}
