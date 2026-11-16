package security

import (
	"bufio"
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"encoding/binary"
	"errors"
	"io"
	"math"
	"os"
)

// Vault file format, for media at rest:
//
//	header : "OCV1" (4 bytes) || file prefix (8 random bytes)
//	chunks : AES-256-GCM sealed plaintext chunks of up to 64 KiB, each followed by its 16-byte tag
//
// Chunk nonce = file prefix (8 bytes) || chunk index (4 bytes, big-endian), so nonces never repeat
// under one key. Each file gets a fresh random 32-byte key. Every chunk except the last is sealed
// with AAD 0x00 and the last with 0x01. Dropping the final chunk therefore fails authentication even
// when the cut falls on a chunk boundary, and so does appending data after it. An empty file is a
// single empty final chunk. Memory use is bounded by one chunk, whatever the file size.
const (
	vaultMagic      = "OCV1"
	vaultPrefixSize = 8
	vaultHeaderSize = len(vaultMagic) + vaultPrefixSize
	vaultChunkSize  = 64 << 10
	vaultTagSize    = 16
	vaultKeySize    = 32
	aadMiddle       = 0x00
	aadFinal        = 0x01
)

var errVaultAuth = errors.New("vault: file is corrupt, truncated or the key is wrong")

// EncryptFile encrypts the file src into a new file dst with a fresh random key, which it returns.
// dst must not exist. On failure dst is removed.
func EncryptFile(src, dst string) (key []byte, err error) {
	key = make([]byte, vaultKeySize)
	if _, err := rand.Read(key); err != nil {
		return nil, err
	}
	in, err := os.Open(src)
	if err != nil {
		return nil, err
	}
	defer in.Close()
	out, err := os.OpenFile(dst, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o600)
	if err != nil {
		return nil, err
	}
	if err := sealStream(in, out, key); err != nil {
		return nil, closeAndRemove(out, dst, err)
	}
	if err := out.Sync(); err != nil {
		return nil, closeAndRemove(out, dst, err)
	}
	if err := out.Close(); err != nil {
		os.Remove(dst)
		return nil, err
	}
	return key, nil
}

// OpenReader returns a reader that decrypts the vault file at path one chunk at a time. An
// authentication failure is returned from Read, so unverified bytes are never handed out. The
// caller must Close the reader.
func OpenReader(path string, key []byte) (io.ReadCloser, error) {
	if len(key) != vaultKeySize {
		return nil, errors.New("vault: key must be 32 bytes")
	}
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	r, err := newVaultReader(f, key)
	if err != nil {
		f.Close()
		return nil, err
	}
	r.closer = f
	return r, nil
}

// DecryptToFile decrypts the vault file src into dst, which is created with mode 0600 or truncated
// if it exists. On failure dst is removed, so no partial plaintext is left behind.
func DecryptToFile(src, dst string, key []byte) error {
	in, err := OpenReader(src, key)
	if err != nil {
		return err
	}
	defer in.Close()
	out, err := os.OpenFile(dst, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, 0o600)
	if err != nil {
		return err
	}
	if _, err := io.Copy(out, in); err != nil {
		return closeAndRemove(out, dst, err)
	}
	if err := out.Close(); err != nil {
		os.Remove(dst)
		return err
	}
	return nil
}

func closeAndRemove(f *os.File, path string, cause error) error {
	f.Close()
	os.Remove(path)
	return cause
}

// sealStream writes the vault encoding of r to w.
func sealStream(r io.Reader, w io.Writer, key []byte) error {
	aead, err := newVaultAEAD(key)
	if err != nil {
		return err
	}
	var prefix [vaultPrefixSize]byte
	if _, err := rand.Read(prefix[:]); err != nil {
		return err
	}
	if _, err := w.Write(append([]byte(vaultMagic), prefix[:]...)); err != nil {
		return err
	}

	in := bufio.NewReaderSize(r, vaultChunkSize)
	plain := make([]byte, vaultChunkSize)
	sealed := make([]byte, 0, vaultChunkSize+vaultTagSize)
	for index := uint64(0); ; index++ {
		if index > math.MaxUint32 {
			return errors.New("vault: file too large")
		}
		n, final, err := readChunk(in, plain)
		if err != nil {
			return err
		}
		sealed = aead.Seal(sealed[:0], vaultNonce(prefix[:], uint32(index)), plain[:n], aadFor(final))
		if _, err := w.Write(sealed); err != nil {
			return err
		}
		if final {
			return nil
		}
	}
}

// readChunk fills buf from r and reports whether this is the last chunk: a short read, or a full
// read with nothing after it.
func readChunk(r *bufio.Reader, buf []byte) (n int, final bool, err error) {
	n, err = io.ReadFull(r, buf)
	switch {
	case err == nil:
		_, peekErr := r.Peek(1)
		if errors.Is(peekErr, io.EOF) {
			return n, true, nil
		}
		return n, false, peekErr
	case errors.Is(err, io.EOF), errors.Is(err, io.ErrUnexpectedEOF):
		return n, true, nil
	default:
		return 0, false, err
	}
}

// vaultReader decrypts a vault stream one chunk at a time.
type vaultReader struct {
	in       *bufio.Reader
	aead     cipher.AEAD
	prefix   []byte
	index    uint64
	sealed   []byte // ciphertext of the current chunk, reused
	plainBuf []byte // plaintext storage, reused
	plain    []byte // unread plaintext of the current chunk
	err      error  // sticky: io.EOF after the final chunk, or the first failure
	closer   io.Closer
}

func newVaultReader(r io.Reader, key []byte) (*vaultReader, error) {
	aead, err := newVaultAEAD(key)
	if err != nil {
		return nil, err
	}
	in := bufio.NewReaderSize(r, vaultChunkSize+vaultTagSize)
	header := make([]byte, vaultHeaderSize)
	if _, err := io.ReadFull(in, header); err != nil || string(header[:len(vaultMagic)]) != vaultMagic {
		return nil, errVaultAuth
	}
	return &vaultReader{
		in:       in,
		aead:     aead,
		prefix:   header[len(vaultMagic):],
		sealed:   make([]byte, vaultChunkSize+vaultTagSize),
		plainBuf: make([]byte, 0, vaultChunkSize),
	}, nil
}

func (v *vaultReader) Read(p []byte) (int, error) {
	for len(v.plain) == 0 {
		if v.err != nil {
			return 0, v.err
		}
		v.err = v.nextChunk()
	}
	n := copy(p, v.plain)
	v.plain = v.plain[n:]
	return n, nil
}

// Close closes the underlying file when the reader was created by OpenReader.
func (v *vaultReader) Close() error {
	if v.closer == nil {
		return nil
	}
	return v.closer.Close()
}

// nextChunk opens the next chunk into v.plain. It returns io.EOF after the final chunk.
func (v *vaultReader) nextChunk() error {
	if v.index > math.MaxUint32 {
		return errVaultAuth
	}
	n, final, err := readChunk(v.in, v.sealed)
	if err != nil {
		return err
	}
	plain, err := v.aead.Open(v.plainBuf[:0], vaultNonce(v.prefix, uint32(v.index)), v.sealed[:n], aadFor(final))
	if err != nil {
		return errVaultAuth
	}
	v.index++
	v.plainBuf = plain
	v.plain = plain
	if final {
		return io.EOF
	}
	return nil
}

func newVaultAEAD(key []byte) (cipher.AEAD, error) {
	block, err := aes.NewCipher(key)
	if err != nil {
		return nil, err
	}
	return cipher.NewGCM(block)
}

func vaultNonce(prefix []byte, index uint32) []byte {
	nonce := make([]byte, 0, vaultPrefixSize+4)
	nonce = append(nonce, prefix...)
	return binary.BigEndian.AppendUint32(nonce, index)
}

func aadFor(final bool) []byte {
	if final {
		return []byte{aadFinal}
	}
	return []byte{aadMiddle}
}
