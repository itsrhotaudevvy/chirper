package engine

import (
	"sync"
	"testing"
	"time"

	"gopkg.in/yaml.v3"
	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/status"
)

type collector struct {
	mu     sync.Mutex
	events []string
}

func (c *collector) OnEvent(kind, payload string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.events = append(c.events, kind+":"+payload)
}

func (c *collector) snapshot() []string {
	c.mu.Lock()
	defer c.mu.Unlock()
	return append([]string(nil), c.events...)
}

func TestDispatcherCoalescesKeyedEvents(t *testing.T) {
	col := &collector{}
	d := newDispatcher(col)
	go d.run()
	defer d.stop()

	version := "first"
	d.queue("chat:a", func() (string, string, bool) { return "chat", version, true })
	version = "second" // the builder reads the state at delivery time
	d.queue("chat:a", func() (string, string, bool) { return "chat", version, true })
	d.queue("chat:b", func() (string, string, bool) { return "chat", "b", true })
	d.queue("chat:c", func() (string, string, bool) { return "", "", false })
	d.emit("core_error", `{"message":"x"}`)
	d.emit("core_error", `{"message":"y"}`)

	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) && len(col.snapshot()) < 4 {
		time.Sleep(5 * time.Millisecond)
	}
	time.Sleep(100 * time.Millisecond)
	got := col.snapshot()
	want := []string{"chat:second", "chat:b", "core_error:{\"message\":\"x\"}", "core_error:{\"message\":\"y\"}"}
	if len(got) != len(want) {
		t.Fatalf("delivered %v, want %v", got, want)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Errorf("event %d = %q, want %q", i, got[i], want[i])
		}
	}
}

func TestDropTelemetryRemovesOnlyTelemetryKeys(t *testing.T) {
	const example = `
proxy:
    type: disabled
analytics:
    token: abc
display_name: keep
nested:
    proxy: keep-nested
`
	var doc yaml.Node
	if err := yaml.Unmarshal([]byte(example), &doc); err != nil {
		t.Fatal(err)
	}
	dropTelemetry(&doc)
	var out map[string]any
	if err := doc.Decode(&out); err != nil {
		t.Fatal(err)
	}
	if _, ok := out["proxy"]; ok {
		t.Error("top-level proxy kept")
	}
	if _, ok := out["analytics"]; ok {
		t.Error("analytics kept")
	}
	if out["display_name"] != "keep" {
		t.Errorf("display_name = %v", out["display_name"])
	}
	if _, ok := out["nested"].(map[string]any)["proxy"]; !ok {
		t.Error("nested proxy key was removed; only top-level telemetry keys may go")
	}
}

func TestAccountStateMapping(t *testing.T) {
	cases := map[status.BridgeStateEvent]string{
		"":                              "CONNECTING",
		status.StateBackfilling:         "CONNECTED",
		status.StateConnected:           "CONNECTED",
		status.StateBadCredentials:      "BAD_CREDENTIALS",
		status.StateTransientDisconnect: "TRANSIENT_DISCONNECT",
		status.StateLoggedOut:           "LOGGED_OUT",
	}
	for in, want := range cases {
		if got := accountState(in); got != want {
			t.Errorf("accountState(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestSettingsDefaultsAndPerNetworkFlags(t *testing.T) {
	m := map[string]bool{"*": true, "signal": false}
	if allows(m, "signal") {
		t.Error("explicit per-network false was overridden by the default")
	}
	if !allows(m, "whatsapp") {
		t.Error("wildcard default not applied")
	}
	if !allows(nil, "anything") {
		t.Error("missing flags must default to enabled")
	}
	s := defaultSettings()
	if s.DemoNetwork || !s.SendTyping["*"] || !s.SendReadReceipts["*"] {
		t.Errorf("default settings = %+v", s)
	}
}

func TestStepJSONUsesContractKeys(t *testing.T) {
	step := &bridgev2.LoginStep{
		Type:         bridgev2.LoginStepTypeUserInput,
		StepID:       "s1",
		Instructions: "enter",
		UserInputParams: &bridgev2.LoginUserInputParams{Fields: []bridgev2.LoginInputDataField{{
			Type: bridgev2.LoginInputFieldTypePhoneNumber, ID: "phone", Name: "Phone",
		}}},
	}
	got := mustJSON(stepJSON(step))
	want := `{"type":"user_input","stepId":"s1","instructions":"enter","userInput":{"fields":[{"id":"phone","name":"Phone","type":"phone_number","description":"","options":[],"pattern":""}]}}`
	if got != want {
		t.Errorf("stepJSON = %s\nwant %s", got, want)
	}
}
