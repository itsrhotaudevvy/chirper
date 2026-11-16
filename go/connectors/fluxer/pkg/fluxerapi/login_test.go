// SPDX-License-Identifier: AGPL-3.0-or-later

package fluxerapi

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"net/http"
	"strings"
	"testing"
)

// altchaTestChallenge is a cheap challenge: a one-byte prefix needs ~256 attempts at cost 1.
func altchaTestChallenge() string {
	return `{"parameters":{"algorithm":"PBKDF2/SHA-256","nonce":"0102","salt":"0a0b","cost":1,"keyLength":16,"keyPrefix":"00","keySignature":"ff","expiresAt":4102444800},"signature":"abcd"}`
}

func TestLoginSolvesCaptchaAndRetries(t *testing.T) {
	var attempts int
	c := newTestClient(t, func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/v1/auth/login" {
			t.Errorf("path = %q", r.URL.Path)
		}
		attempts++
		if attempts == 1 {
			w.WriteHeader(http.StatusBadRequest)
			_, _ = w.Write([]byte(`{"code":"CAPTCHA_REQUIRED","message":"c","captcha_provider":"altcha","altcha_challenge":` + altchaTestChallenge() + `}`))
			return
		}
		token := r.Header.Get("X-Captcha-Token")
		if token == "" {
			t.Fatal("second attempt has no X-Captcha-Token")
		}
		var body LoginRequest
		if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
			t.Fatal(err)
		}
		if body.Login != "me@example.com" || body.Password != "hunter2hunter2" {
			t.Errorf("body = %+v", body)
		}
		_, _ = w.Write([]byte(`{"token":"session-token","user_id":"7","user":{"id":"7","username":"me","discriminator":"0001"}}`))
	}, Credential{})

	res, err := c.Login(context.Background(), LoginRequest{Login: "me@example.com", Password: "hunter2hunter2"})
	if err != nil {
		t.Fatal(err)
	}
	if res.Token != "session-token" || res.MFA {
		t.Fatalf("res = %+v", res)
	}
	if attempts != 2 {
		t.Fatalf("attempts = %d", attempts)
	}
}

func TestSolveAltchaProducesVerifiableToken(t *testing.T) {
	raw := json.RawMessage(altchaTestChallenge())
	token, err := SolveAltcha(context.Background(), raw)
	if err != nil {
		t.Fatal(err)
	}
	decoded, err := base64.StdEncoding.DecodeString(token)
	if err != nil {
		t.Fatal(err)
	}
	var payload struct {
		Challenge struct {
			Parameters struct {
				Nonce string `json:"nonce"`
			} `json:"parameters"`
			Signature string `json:"signature"`
		} `json:"challenge"`
		Solution struct {
			Counter    uint32 `json:"counter"`
			DerivedKey string `json:"derivedKey"`
		} `json:"solution"`
	}
	if err := json.Unmarshal(decoded, &payload); err != nil {
		t.Fatal(err)
	}
	if payload.Challenge.Signature != "abcd" || payload.Challenge.Parameters.Nonce != "0102" {
		t.Fatalf("challenge not copied: %+v", payload.Challenge)
	}
	if !strings.HasPrefix(payload.Solution.DerivedKey, "00") {
		t.Fatalf("derived key %s does not start with prefix 00", payload.Solution.DerivedKey)
	}
}

func TestSolveAltchaRejectsUnsupportedAlgorithm(t *testing.T) {
	bad := json.RawMessage(`{"parameters":{"algorithm":"SCRYPT","nonce":"01","salt":"01","cost":1,"keyLength":8,"keyPrefix":"00"},"signature":"x"}`)
	if _, err := SolveAltcha(context.Background(), bad); err == nil {
		t.Fatal("expected error")
	}
}

func TestLoginTOTPSendsTicketAndCode(t *testing.T) {
	c := newTestClient(t, func(w http.ResponseWriter, r *http.Request) {
		var body map[string]string
		_ = json.NewDecoder(r.Body).Decode(&body)
		if body["ticket"] != "tk" || body["code"] != "123456" {
			t.Errorf("body = %v", body)
		}
		_, _ = w.Write([]byte(`{"token":"after-mfa","user_id":"7","user":{"id":"7","username":"me","discriminator":"0001"}}`))
	}, Credential{})
	res, err := c.LoginTOTP(context.Background(), "tk", "123456")
	if err != nil {
		t.Fatal(err)
	}
	if res.Token != "after-mfa" {
		t.Fatalf("token = %q", res.Token)
	}
}
