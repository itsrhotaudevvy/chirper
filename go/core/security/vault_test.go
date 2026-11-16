package security

import (
	"bytes"
	"crypto/rand"
	"io"
	"os"
	"path/filepath"
	"testing"
)

const vaultSealedChunk = vaultChunkSize + vaultTagSize

func randomBytes(t *testing.T, n int) []byte {
	t.Helper()
	b := make([]byte, n)
	if _, err := rand.Read(b); err != nil {
		t.Fatal(err)
	}
	return b
}

func writeFile(t *testing.T, path string, data []byte) {
	t.Helper()
	if err := os.WriteFile(path, data, 0o600); err != nil {
		t.Fatal(err)
	}
}

// encryptBytes encrypts data through EncryptFile and returns the ciphertext and key.
func encryptBytes(t *testing.T, data []byte) ([]byte, []byte) {
	t.Helper()
	dir := t.TempDir()
	src := filepath.Join(dir, "plain")
	dst := filepath.Join(dir, "sealed")
	writeFile(t, src, data)
	key, err := EncryptFile(src, dst)
	if err != nil {
		t.Fatal(err)
	}
	sealed, err := os.ReadFile(dst)
	if err != nil {
		t.Fatal(err)
	}
	return sealed, key
}

// decryptBytes writes sealed to a temp file and runs DecryptToFile on it.
func decryptBytes(t *testing.T, sealed, key []byte) ([]byte, error) {
	t.Helper()
	dir := t.TempDir()
	src := filepath.Join(dir, "sealed")
	dst := filepath.Join(dir, "plain")
	writeFile(t, src, sealed)
	if err := DecryptToFile(src, dst, key); err != nil {
		if _, statErr := os.Stat(dst); statErr == nil {
			t.Errorf("DecryptToFile left %s behind after failure", dst)
		}
		return nil, err
	}
	return os.ReadFile(dst)
}

func TestVaultRoundTrip(t *testing.T) {
	sizes := []int{0, 1, vaultChunkSize - 1, vaultChunkSize, vaultChunkSize + 1, 3*vaultChunkSize + 17}
	for _, size := range sizes {
		data := randomBytes(t, size)
		sealed, key := encryptBytes(t, data)
		if len(key) != vaultKeySize {
			t.Fatalf("size %d: key length %d", size, len(key))
		}

		got, err := decryptBytes(t, sealed, key)
		if err != nil {
			t.Fatalf("size %d: decrypt: %v", size, err)
		}
		if !bytes.Equal(got, data) {
			t.Fatalf("size %d: round trip mismatch", size)
		}

		// The streaming reader must return the same bytes as DecryptToFile.
		path := filepath.Join(t.TempDir(), "sealed")
		writeFile(t, path, sealed)
		r, err := OpenReader(path, key)
		if err != nil {
			t.Fatal(err)
		}
		streamed, err := io.ReadAll(r)
		r.Close()
		if err != nil || !bytes.Equal(streamed, data) {
			t.Fatalf("size %d: OpenReader mismatch: %v", size, err)
		}
	}
}

func TestVaultCiphertextHidesPlaintext(t *testing.T) {
	data := bytes.Repeat([]byte("needle-plaintext-"), 10000)
	sealed, _ := encryptBytes(t, data)
	if bytes.Contains(sealed, []byte("needle-plaintext-")) {
		t.Fatal("ciphertext contains plaintext")
	}
	if string(sealed[:len(vaultMagic)]) != vaultMagic {
		t.Fatal("missing vault magic")
	}
}

func TestVaultEncryptionIsRandomized(t *testing.T) {
	data := randomBytes(t, 1000)
	a, keyA := encryptBytes(t, data)
	b, keyB := encryptBytes(t, data)
	if bytes.Equal(keyA, keyB) || bytes.Equal(a, b) {
		t.Fatal("two encryptions of the same data reused key or nonce material")
	}
}

func TestVaultRejectsWrongKey(t *testing.T) {
	sealed, key := encryptBytes(t, randomBytes(t, 100))
	wrong := append([]byte(nil), key...)
	wrong[0] ^= 0xff
	if _, err := decryptBytes(t, sealed, wrong); err == nil {
		t.Fatal("decrypt with wrong key succeeded")
	}
}

func TestVaultRejectsTampering(t *testing.T) {
	// Three chunks: two full and one half-full final chunk.
	sealed, key := encryptBytes(t, randomBytes(t, 2*vaultChunkSize+vaultChunkSize/2))
	header := vaultHeaderSize
	cases := map[string]int{
		"file prefix":       len(vaultMagic) + 1,
		"first chunk body":  header + 5,
		"second chunk body": header + vaultSealedChunk + 5,
		"final chunk tag":   len(sealed) - 1,
		"final chunk body":  len(sealed) - vaultTagSize - 1,
	}
	for name, offset := range cases {
		t.Run(name, func(t *testing.T) {
			tampered := append([]byte(nil), sealed...)
			tampered[offset] ^= 0x01
			if _, err := decryptBytes(t, tampered, key); err == nil {
				t.Fatalf("tampering with %s was not detected", name)
			}
		})
	}
}

func TestVaultRejectsTruncation(t *testing.T) {
	// Three chunks: two full and one half-full final chunk.
	sealed, key := encryptBytes(t, randomBytes(t, 2*vaultChunkSize+vaultChunkSize/2))
	header := vaultHeaderSize
	cases := map[string]int{
		"inside header":                  header / 2,
		"header only, no chunks":         header,
		"after first chunk":              header + vaultSealedChunk,
		"at boundary before final chunk": header + 2*vaultSealedChunk,
		"inside final chunk":             len(sealed) - 3,
		"inside final chunk tag":         len(sealed) - 1,
	}
	for name, size := range cases {
		t.Run(name, func(t *testing.T) {
			if _, err := decryptBytes(t, sealed[:size], key); err == nil {
				t.Fatalf("truncation %s was not detected", name)
			}
		})
	}
}

func TestVaultRejectsTruncationAtFinalChunkBoundary(t *testing.T) {
	// Exactly two full chunks, so the last chunk is full and flagged final. Cutting it off leaves a
	// middle chunk that is now read as final, and must not authenticate.
	sealed, key := encryptBytes(t, randomBytes(t, 2*vaultChunkSize))
	if _, err := decryptBytes(t, sealed[:vaultHeaderSize+vaultSealedChunk], key); err == nil {
		t.Fatal("dropping the final chunk on a boundary was not detected")
	}
}

func TestVaultRejectsAppendedData(t *testing.T) {
	sealed, key := encryptBytes(t, randomBytes(t, 2*vaultChunkSize))
	appended := append(append([]byte(nil), sealed...), 0x00)
	if _, err := decryptBytes(t, appended, key); err == nil {
		t.Fatal("appended data was not detected")
	}
}

func TestVaultRejectsReorderedChunks(t *testing.T) {
	sealed, key := encryptBytes(t, randomBytes(t, 3*vaultChunkSize))
	first := sealed[vaultHeaderSize : vaultHeaderSize+vaultSealedChunk]
	second := sealed[vaultHeaderSize+vaultSealedChunk : vaultHeaderSize+2*vaultSealedChunk]
	swapped := append([]byte(nil), sealed[:vaultHeaderSize]...)
	swapped = append(swapped, second...)
	swapped = append(swapped, first...)
	swapped = append(swapped, sealed[vaultHeaderSize+2*vaultSealedChunk:]...)
	if _, err := decryptBytes(t, swapped, key); err == nil {
		t.Fatal("reordered chunks were not detected")
	}
}

func TestEncryptFileRefusesExistingDestination(t *testing.T) {
	dir := t.TempDir()
	src := filepath.Join(dir, "plain")
	dst := filepath.Join(dir, "sealed")
	writeFile(t, src, []byte("new"))
	writeFile(t, dst, []byte("keep me"))
	if _, err := EncryptFile(src, dst); err == nil {
		t.Fatal("EncryptFile overwrote an existing file")
	}
	got, err := os.ReadFile(dst)
	if err != nil || string(got) != "keep me" {
		t.Fatalf("existing destination changed: %q, %v", got, err)
	}
}

func TestOpenReaderRejectsBadKeyLength(t *testing.T) {
	path := filepath.Join(t.TempDir(), "sealed")
	writeFile(t, path, []byte("x"))
	if _, err := OpenReader(path, make([]byte, 16)); err == nil {
		t.Fatal("OpenReader accepted a 16-byte key")
	}
}
