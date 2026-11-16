// SPDX-License-Identifier: AGPL-3.0-or-later

package fluxerapi

import (
	"bytes"
	"context"
	"crypto/pbkdf2"
	"crypto/sha256"
	"encoding/base64"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"runtime"
	"sync"
)

// altchaAlgorithm is the only ALTCHA algorithm Fluxer issues (docs: topics/captcha.md).
const altchaAlgorithm = "PBKDF2/SHA-256"

// ErrCaptchaUnsolvable means the challenge has no solution in the uint32 counter space.
var ErrCaptchaUnsolvable = errors.New("fluxer: CAPTCHA challenge has no solution")

type altchaParams struct {
	Algorithm string `json:"algorithm"`
	Nonce     string `json:"nonce"`
	Salt      string `json:"salt"`
	Cost      int    `json:"cost"`
	KeyLength int    `json:"keyLength"`
	KeyPrefix string `json:"keyPrefix"`
}

type altchaChallenge struct {
	Parameters altchaParams `json:"parameters"`
}

type altchaSolution struct {
	Counter    uint32 `json:"counter"`
	DerivedKey string `json:"derivedKey"`
}

// SolveAltcha solves an ALTCHA v2 PBKDF2/SHA-256 challenge, as sent in altcha_challenge, and
// returns the value for the X-Captcha-Token header. The search is the one the Fluxer server
// verifies (fluxer_api/pkgs/captcha/src/providers/AltchaProvider.ts): the password for counter n
// is nonce || uint32_BE(n), and the derived key must start with keyPrefix. The reference
// implementation is fluxer_rexctl/internal/setup/captcha.go.
func SolveAltcha(ctx context.Context, challenge json.RawMessage) (string, error) {
	var ch altchaChallenge
	if err := json.Unmarshal(challenge, &ch); err != nil {
		return "", fmt.Errorf("fluxer: malformed CAPTCHA challenge: %w", err)
	}
	p := ch.Parameters
	nonce, nerr := hex.DecodeString(p.Nonce)
	salt, serr := hex.DecodeString(p.Salt)
	prefix, perr := hex.DecodeString(p.KeyPrefix)
	if p.Algorithm != altchaAlgorithm || nerr != nil || serr != nil || perr != nil ||
		p.Cost <= 0 || p.KeyLength <= 0 || len(prefix) == 0 || len(prefix) > p.KeyLength {
		return "", errors.New("fluxer: unsupported CAPTCHA challenge parameters")
	}
	counter, key, err := searchAltcha(ctx, nonce, salt, prefix, p.Cost, p.KeyLength)
	if err != nil {
		return "", err
	}
	body, err := json.Marshal(struct {
		Challenge json.RawMessage `json:"challenge"`
		Solution  altchaSolution  `json:"solution"`
	}{
		Challenge: challenge,
		Solution:  altchaSolution{Counter: counter, DerivedKey: hex.EncodeToString(key)},
	})
	if err != nil {
		return "", err
	}
	return base64.StdEncoding.EncodeToString(body), nil
}

// searchAltcha splits the counter space between CPU workers (worker w tries w, w+N, w+2N, ...)
// and returns the first counter whose derived key matches the prefix.
func searchAltcha(parent context.Context, nonce, salt, prefix []byte, cost, keyLength int) (uint32, []byte, error) {
	ctx, cancel := context.WithCancel(parent)
	defer cancel()
	workers := runtime.NumCPU()
	var (
		mu       sync.Mutex
		foundN   uint32
		foundKey []byte
		failure  error
		wg       sync.WaitGroup
	)
	record := func(n uint32, key []byte, err error) {
		mu.Lock()
		defer mu.Unlock()
		if err != nil {
			if failure == nil {
				failure = err
			}
		} else if foundKey == nil {
			foundN, foundKey = n, key
		}
		cancel()
	}
	for w := 0; w < workers; w++ {
		wg.Add(1)
		go func(start uint64) {
			defer wg.Done()
			password := make([]byte, len(nonce)+4)
			copy(password, nonce)
			for n := start; n <= math.MaxUint32 && ctx.Err() == nil; n += uint64(workers) {
				binary.BigEndian.PutUint32(password[len(nonce):], uint32(n))
				key, err := pbkdf2.Key(sha256.New, string(password), salt, cost, keyLength)
				if err != nil {
					record(0, nil, err)
					return
				}
				if bytes.HasPrefix(key, prefix) {
					record(uint32(n), key, nil)
					return
				}
			}
		}(uint64(w))
	}
	wg.Wait()
	switch {
	case foundKey != nil:
		return foundN, foundKey, nil
	case failure != nil:
		return 0, nil, failure
	case parent.Err() != nil:
		return 0, nil, parent.Err()
	default:
		return 0, nil, ErrCaptchaUnsolvable
	}
}
