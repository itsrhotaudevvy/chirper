// SPDX-License-Identifier: AGPL-3.0-or-later

package fluxerapi

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"mime"
	"mime/multipart"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync/atomic"
	"testing"
)

func newTestClient(t *testing.T, h http.HandlerFunc, cred Credential) *Client {
	t.Helper()
	srv := httptest.NewServer(h)
	t.Cleanup(srv.Close)
	return NewClient(srv.URL, cred, srv.Client())
}

func TestMeSendsBotAuthorizationAndUserAgent(t *testing.T) {
	c := newTestClient(t, func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/v1/users/@me" {
			t.Errorf("path = %q", r.URL.Path)
		}
		if got := r.Header.Get("Authorization"); got != "Bot 123.secret" {
			t.Errorf("Authorization = %q", got)
		}
		if got := r.Header.Get("User-Agent"); got != UserAgent {
			t.Errorf("User-Agent = %q", got)
		}
		_, _ = w.Write([]byte(`{"id":"42","username":"bot","discriminator":"0001","bot":true}`))
	}, Credential{Token: "123.secret", Bot: true})

	u, err := c.Me(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if u.ID != "42" || !u.Bot {
		t.Fatalf("unexpected user %+v", u)
	}
}

func TestUserTokenIsSentRawAndDoesNotRetryGlobalDenial(t *testing.T) {
	var calls atomic.Int32
	c := newTestClient(t, func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		if got := r.Header.Get("Authorization"); got != "user-token" {
			t.Errorf("Authorization = %q", got)
		}
		w.Header().Set("Retry-After", "1")
		w.WriteHeader(http.StatusTooManyRequests)
		_, _ = w.Write([]byte(`{"code":"RATE_LIMITED","message":"slow","global":true,"retry_after":0.001}`))
	}, Credential{Token: "user-token"})

	_, err := c.Me(context.Background())
	if !HasCode(err, "RATE_LIMITED") {
		t.Fatalf("err = %v, want RATE_LIMITED", err)
	}
	if n := calls.Load(); n != 1 {
		t.Fatalf("calls = %d, want 1 (global denial of a user token must not be retried)", n)
	}
}

func TestRateLimitedRouteIsRetried(t *testing.T) {
	var calls atomic.Int32
	c := newTestClient(t, func(w http.ResponseWriter, r *http.Request) {
		if calls.Add(1) == 1 {
			w.WriteHeader(http.StatusTooManyRequests)
			_, _ = w.Write([]byte(`{"code":"RATE_LIMITED","message":"slow","global":false,"retry_after":0.001}`))
			return
		}
		_, _ = w.Write([]byte(`{"id":"1","username":"x","discriminator":"0001"}`))
	}, Credential{Token: "bot.tok", Bot: true})

	if _, err := c.Me(context.Background()); err != nil {
		t.Fatalf("Me after retry: %v", err)
	}
	if n := calls.Load(); n != 2 {
		t.Fatalf("calls = %d, want 2", n)
	}
}

func TestCreateMessageWithFileIsMultipart(t *testing.T) {
	c := newTestClient(t, func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/v1/channels/10/messages" || r.Method != http.MethodPost {
			t.Errorf("%s %s", r.Method, r.URL.Path)
		}
		mediaType, params, err := mime.ParseMediaType(r.Header.Get("Content-Type"))
		if err != nil || mediaType != "multipart/form-data" {
			t.Fatalf("content type = %q (%v)", r.Header.Get("Content-Type"), err)
		}
		mr := multipart.NewReader(r.Body, params["boundary"])
		var sawPayload, sawFile bool
		for {
			part, err := mr.NextPart()
			if err == io.EOF {
				break
			}
			if err != nil {
				t.Fatal(err)
			}
			data, _ := io.ReadAll(part)
			switch part.FormName() {
			case "payload_json":
				sawPayload = true
				var body struct {
					Content     string `json:"content"`
					Attachments []struct {
						ID       int    `json:"id"`
						Filename string `json:"filename"`
					} `json:"attachments"`
				}
				if err := json.Unmarshal(data, &body); err != nil {
					t.Fatal(err)
				}
				if body.Content != "look" || len(body.Attachments) != 1 || body.Attachments[0].Filename != "a.png" {
					t.Errorf("payload = %+v", body)
				}
			case "files[0]":
				sawFile = true
				if string(data) != "PNGDATA" {
					t.Errorf("file bytes = %q", data)
				}
			}
		}
		if !sawPayload || !sawFile {
			t.Errorf("payload=%v file=%v", sawPayload, sawFile)
		}
		_, _ = w.Write([]byte(`{"id":"99","channel_id":"10","author":{"id":"1","username":"b"},"content":"look","timestamp":"2026-08-31T09:14:02.000Z"}`))
	}, Credential{Token: "t", Bot: true})

	content := "look"
	msg, err := c.CreateMessage(context.Background(), "10",
		MessageBody{Content: &content},
		Upload{Meta: AttachmentMeta{ID: 0, Filename: "a.png", ContentType: "image/png"}, Data: []byte("PNGDATA")})
	if err != nil {
		t.Fatal(err)
	}
	if msg.ID != "99" {
		t.Fatalf("id = %q", msg.ID)
	}
}

func TestCaptchaErrorIsParsed(t *testing.T) {
	c := newTestClient(t, func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusBadRequest)
		_, _ = w.Write([]byte(`{"code":"CAPTCHA_REQUIRED","message":"x","captcha_provider":"altcha","altcha_challenge":{"parameters":{"algorithm":"PBKDF2/SHA-256"},"signature":"ab"}}`))
	}, Credential{})
	_, err := c.Me(context.Background())
	var apiErr *APIError
	if !errors.As(err, &apiErr) || !apiErr.isCaptchaRequired() {
		t.Fatalf("err = %#v", err)
	}
	if !strings.Contains(string(apiErr.CaptchaChallenge), `"signature":"ab"`) {
		t.Fatalf("challenge = %s", apiErr.CaptchaChallenge)
	}
}

func TestCredentialStringHidesToken(t *testing.T) {
	s := Credential{Token: "super-secret", Bot: true}.String()
	if strings.Contains(s, "super-secret") {
		t.Fatalf("String leaked token: %s", s)
	}
}
