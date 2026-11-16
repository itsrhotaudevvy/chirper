package security

import (
	"bytes"
	"encoding/json"
	"strings"
	"testing"
)

func TestIsSensitiveField(t *testing.T) {
	for _, name := range []string{"body", "Body", "text", "event_text", "eventText", "token", "accessToken",
		"cookie", "cookies", "password", "phone", "phoneNumber", "email", "key", "api_key", "secret",
		"displayName", "display_name", "authorization"} {
		if !IsSensitiveField(name) {
			t.Errorf("%q not treated as sensitive", name)
		}
	}
	for _, name := range []string{"level", "message", "network", "roomId", "context", "error", "time", "monkey"} {
		if IsSensitiveField(name) {
			t.Errorf("%q wrongly treated as sensitive", name)
		}
	}
}

func TestRedactingWriterDropsSensitiveFields(t *testing.T) {
	var out bytes.Buffer
	w := NewRedactingWriter(&out, "/Users/alice/Library/Chirper")
	line := `{"level":"info","message":"sent","body":"hello secret","token":"abc","nested":{"cookie":"x","n":1},` +
		`"path":"/Users/alice/Library/Chirper/media/f","list":[{"password":"p"}]}` + "\n"
	n, err := w.Write([]byte(line))
	if err != nil || n != len(line) {
		t.Fatalf("Write = %d, %v", n, err)
	}
	got := out.String()
	for _, secret := range []string{"hello secret", `"abc"`, `"x"`, `"p"`, "alice"} {
		if strings.Contains(got, secret) {
			t.Errorf("output still contains %q: %s", secret, got)
		}
	}
	var obj map[string]any
	if err := json.Unmarshal([]byte(got), &obj); err != nil {
		t.Fatalf("output is not JSON: %v", err)
	}
	if obj["body"] != "[redacted]" || obj["message"] != "sent" {
		t.Errorf("unexpected output: %s", got)
	}
	if obj["path"] != "<data>/media/f" {
		t.Errorf("path not shortened: %v", obj["path"])
	}
}

func TestRedactingWriterDropsUnparseableLines(t *testing.T) {
	var out bytes.Buffer
	w := NewRedactingWriter(&out, "")
	if _, err := w.Write([]byte("body: hello not json\n")); err != nil {
		t.Fatal(err)
	}
	if strings.Contains(out.String(), "hello") {
		t.Fatalf("unparseable line leaked: %q", out.String())
	}
}
