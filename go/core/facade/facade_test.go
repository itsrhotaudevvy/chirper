package facade

import (
	"errors"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"

	"chirper.dev/core/engine"
	"chirper.dev/core/security"
)

type eventLog struct {
	mu     sync.Mutex
	events []string
}

func (e *eventLog) OnEvent(kind, payload string) {
	e.mu.Lock()
	defer e.mu.Unlock()
	e.events = append(e.events, kind+" "+payload)
}

func (e *eventLog) count(substr string) int {
	e.mu.Lock()
	defer e.mu.Unlock()
	n := 0
	for _, ev := range e.events {
		if strings.Contains(ev, substr) {
			n++
		}
	}
	return n
}

func openCore(t *testing.T, key []byte, listener EventListener) *Core {
	t.Helper()
	c, err := NewCoreWithKey(t.TempDir(), key, listener)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(c.Stop)
	return c
}

func TestNewCoreWithKeyChecksKeyLength(t *testing.T) {
	for _, n := range []int{1, engine.DBKeyBytes - 1, engine.DBKeyBytes + 1} {
		_, err := NewCoreWithKey(t.TempDir(), make([]byte, n), &eventLog{})
		if err == nil {
			t.Errorf("key of %d bytes accepted", n)
		}
	}
}

func TestNewCoreWithKeyAcceptsNilAndEmptyKeys(t *testing.T) {
	openCore(t, nil, &eventLog{})
	openCore(t, []byte{}, &eventLog{})
}

func TestNewCoreWithKeyAcceptsA32ByteKey(t *testing.T) {
	key := make([]byte, engine.DBKeyBytes)
	for i := range key {
		key[i] = byte(i + 1)
	}
	openCore(t, key, &eventLog{})
}

func TestNewCoreRejectsNilListener(t *testing.T) {
	if _, err := NewCoreWithKey(t.TempDir(), nil, nil); err == nil {
		t.Fatal("nil listener accepted")
	}
}

func TestEncryptionWarningIsLoggedOnce(t *testing.T) {
	dir := t.TempDir()
	key := make([]byte, engine.DBKeyBytes)
	for name, tc := range map[string]struct {
		key  []byte
		want int
	}{
		"no key":   {key: nil, want: 1},
		"with key": {key: key, want: map[bool]int{true: 0, false: 1}[security.EncryptionAvailable]},
	} {
		t.Run(name, func(t *testing.T) {
			sub := filepath.Join(dir, strings.ReplaceAll(name, " ", "-"))
			c, err := NewCoreWithKey(sub, tc.key, &eventLog{})
			if err != nil {
				t.Fatal(err)
			}
			c.Stop()
			// The log file is created lazily: no file means nothing was logged.
			raw, err := os.ReadFile(filepath.Join(sub, "logs", "core.log"))
			if err != nil && !errors.Is(err, os.ErrNotExist) {
				t.Fatal(err)
			}
			if got := strings.Count(string(raw), "NOT encrypted"); got != tc.want {
				t.Errorf("encryption warnings = %d, want %d:\n%s", got, tc.want, raw)
			}
		})
	}
}

func TestInputLimitsAreEnforced(t *testing.T) {
	ev := &eventLog{}
	c := openCore(t, nil, ev)
	room := "!demo.none:chirper.local"
	tooLongText := strings.Repeat("x", maxTextBytes+1)
	atTextLimit := strings.Repeat("x", maxTextBytes)

	if _, err := c.SendText(room, tooLongText, "", ""); err == nil || !strings.Contains(err.Error(), "text is too long") {
		t.Errorf("oversized text error = %v", err)
	}
	if _, err := c.SendText(room, atTextLimit, "", ""); err != nil && strings.Contains(err.Error(), "too long") {
		t.Errorf("text at the limit refused: %v", err)
	}
	if err := c.React(room, "$e", strings.Repeat("k", maxKeyBytes+1)); err == nil {
		t.Error("oversized reaction key accepted")
	}
	if err := c.Edit(room, "$e", tooLongText); err == nil {
		t.Error("oversized edit accepted")
	}
	longID := strings.Repeat("a", maxIDBytes+1)
	if _, err := c.StartLogin(longID, "phone"); err == nil {
		t.Error("oversized network id accepted")
	}
	if _, err := c.MediaFile("mxc://chirper.local/" + longID); err == nil {
		t.Error("oversized mxc accepted")
	}
	if got := c.Search(strings.Repeat("q", maxQueryBytes+1), 10); got != "[]" {
		t.Errorf("oversized search = %q, want []", got)
	}
	if ev.count("core_error ") != 1 || ev.count("too long") != 1 {
		t.Errorf("oversized search was not reported once as a core_error: %v", ev.events)
	}
}

func TestClampLimit(t *testing.T) {
	cases := map[int]int{-5: 1, 0: 1, 1: 1, 42: 42, 500: 500, 501: 500, 1 << 30: 500}
	for in, want := range cases {
		if got := clampLimit(in); got != want {
			t.Errorf("clampLimit(%d) = %d, want %d", in, got, want)
		}
	}
}

func TestParseCookiesCapsPayloadAndDropsControlCharacters(t *testing.T) {
	got, err := parseCookies(`{"sid":"abc","bad":"a\u0000b","tab":"x\ty","ok":"1"}`)
	if err != nil {
		t.Fatal(err)
	}
	if len(got) != 2 || got["sid"] != "abc" || got["ok"] != "1" {
		t.Errorf("cookies = %v, want only the clean entries", got)
	}

	huge := `{"sid":"` + strings.Repeat("a", maxCookieBytes) + `"}`
	if _, err := parseCookies(huge); err == nil {
		t.Error("cookie payload over 64 KiB accepted")
	}
}

func TestInvalidJSONErrorsDoNotEchoSecrets(t *testing.T) {
	secret := `{"password": "hunter2-` + strings.Repeat("z", 8) + `"`
	_, err := parseCookies(secret)
	if err == nil || strings.Contains(err.Error(), "hunter2") {
		t.Fatalf("error = %v, want a generic message without the payload", err)
	}
	_, err = parseStringMap(secret, "login input")
	if err == nil || strings.Contains(err.Error(), "hunter2") {
		t.Fatalf("error = %v, want a generic message without the payload", err)
	}
}
