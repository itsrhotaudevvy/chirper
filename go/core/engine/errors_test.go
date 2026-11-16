package engine_test

import (
	"strings"
	"testing"

	"chirper.dev/core/facade"
)

// TestOperationsFailClearlyWithoutLogins checks the error contract of the facade on an empty core.
func TestOperationsFailClearlyWithoutLogins(t *testing.T) {
	rec := &recorder{}
	c, err := facade.NewCore(t.TempDir(), rec)
	if err != nil {
		t.Fatal(err)
	}
	if err := c.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(c.Stop)

	if _, err := c.StartLogin("telegram", "phone"); err == nil || !strings.Contains(err.Error(), "api_id and api_hash") {
		t.Errorf("telegram login error = %v, want a clear missing-credentials error", err)
	}
	if _, err := c.StartLogin("demo", "demo"); err == nil {
		t.Error("demo login succeeded while the demo network is disabled")
	}
	if _, err := c.SendText("!demo.none:chirper.local", "hi", "", ""); err == nil {
		t.Error("sending to an unknown chat succeeded")
	}
	if got := c.GetChat("!demo.none:chirper.local"); got != "" {
		t.Errorf("GetChat of an unknown chat = %q, want empty", got)
	}
	if got := c.ListChats(""); got != "[]" {
		t.Errorf("ListChats on an empty core = %q", got)
	}
	if _, err := c.SubmitLoginInput("no-such-process", `{}`); err == nil {
		t.Error("submit to an unknown login process succeeded")
	}
	if err := c.SetSettings(`{not json`); err == nil {
		t.Error("invalid settings were accepted")
	}
}
