package main

import (
	"encoding/json"
	"errors"
	"strings"
	"testing"

	"chirper.dev/core/facade"
)

// nullListener discards events.
type nullListener struct{}

func (nullListener) OnEvent(kind, payload string) {}

// newDemoCore opens a core under a temporary directory with the demo network enabled and started.
func newDemoCore(t *testing.T) *facade.Core {
	t.Helper()
	c, err := facade.NewCore(t.TempDir(), nullListener{})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(c.Stop)
	if err := c.SetSettings(`{"demoNetwork":true}`); err != nil {
		t.Fatal(err)
	}
	if err := c.Start(); err != nil {
		t.Fatal(err)
	}
	return c
}

// decodeEnvelope parses an envelope into its ok flag, value and error message.
func decodeEnvelope(t *testing.T, b []byte) (ok bool, value json.RawMessage, msg string) {
	t.Helper()
	var env struct {
		OK    bool            `json:"ok"`
		Value json.RawMessage `json:"value"`
		Error string          `json:"error"`
	}
	if err := json.Unmarshal(b, &env); err != nil {
		t.Fatalf("envelope is not JSON: %s: %v", b, err)
	}
	return env.OK, env.Value, env.Error
}

func TestOkEnvelopeShape(t *testing.T) {
	ok, value, _ := decodeEnvelope(t, okEnvelope(json.RawMessage(`[1,2]`)))
	if !ok || string(value) != "[1,2]" {
		t.Fatalf("ok envelope = ok:%v value:%s", ok, value)
	}
	if got := string(okEnvelope(nil)); got != `{"ok":true,"value":null}` {
		t.Fatalf("nil value envelope = %s", got)
	}
}

func TestErrorEnvelopeShape(t *testing.T) {
	ok, _, msg := decodeEnvelope(t, errorEnvelope(errors.New(`bad "quote"`)))
	if ok || msg != `bad "quote"` {
		t.Fatalf("error envelope = ok:%v error:%q", ok, msg)
	}
}

func TestInvalidValueBecomesErrorEnvelope(t *testing.T) {
	ok, _, msg := decodeEnvelope(t, okEnvelope(json.RawMessage(`{not json`)))
	if ok || msg == "" {
		t.Fatalf("invalid value must become an error envelope, got ok:%v error:%q", ok, msg)
	}
}

func TestDispatchNetworksIsArray(t *testing.T) {
	c := newDemoCore(t)
	value, err := dispatch(c, "networks", []byte("{}"))
	if err != nil {
		t.Fatal(err)
	}
	var networks []struct {
		ID string `json:"id"`
	}
	if err := json.Unmarshal(value, &networks); err != nil {
		t.Fatalf("networks is not a JSON array: %s: %v", value, err)
	}
	found := false
	for _, n := range networks {
		found = found || n.ID == "demo"
	}
	if !found {
		t.Fatalf("demo network not listed: %s", value)
	}
}

func TestDispatchUnknownMethod(t *testing.T) {
	c := newDemoCore(t)
	_, err := dispatch(c, "noSuchMethod", []byte("{}"))
	if err == nil || !strings.Contains(err.Error(), "unknown method") {
		t.Fatalf("unknown method error = %v", err)
	}
}

func TestDispatchRejectsOversizedArgs(t *testing.T) {
	c := newDemoCore(t)
	args := []byte("{" + strings.Repeat(" ", maxArgsBytes) + "}")
	if _, err := dispatch(c, "networks", args); err == nil {
		t.Fatal("args over 1 MiB were accepted")
	}
}

func TestDispatchRejectsBadArgs(t *testing.T) {
	c := newDemoCore(t)
	for _, args := range []string{`[]`, `{"roomId":5}`} {
		if _, err := dispatch(c, "sendText", []byte(args)); err == nil {
			t.Fatalf("sendText accepted args %s", args)
		}
	}
}

func TestDispatchEmptyArgsMeansEmptyObject(t *testing.T) {
	c := newDemoCore(t)
	if _, err := dispatch(c, "accounts", nil); err != nil {
		t.Fatalf("accounts with no args: %v", err)
	}
}

func TestDispatchDemoLoginAndAccounts(t *testing.T) {
	c := newDemoCore(t)
	start, err := dispatch(c, "startLogin", []byte(`{"network":"demo","flowId":"demo"}`))
	if err != nil {
		t.Fatal(err)
	}
	var first struct {
		ProcessID string `json:"processId"`
	}
	if err := json.Unmarshal(start, &first); err != nil || first.ProcessID == "" {
		t.Fatalf("startLogin = %s (%v)", start, err)
	}
	args := []byte(`{"processId":"` + first.ProcessID + `","values":{"name":"tester"}}`)
	if _, err := dispatch(c, "submitLoginInput", args); err != nil {
		t.Fatalf("submitLoginInput: %v", err)
	}
	accounts, err := dispatch(c, "accounts", nil)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(accounts), `"loginId":"demo-tester"`) {
		t.Fatalf("demo account not listed: %s", accounts)
	}
}

func TestDispatchNullForEmptyChat(t *testing.T) {
	c := newDemoCore(t)
	value, err := dispatch(c, "getChat", []byte(`{"roomId":"!missing"}`))
	if err != nil {
		t.Fatal(err)
	}
	if got := string(okEnvelope(value)); got != `{"ok":true,"value":null}` {
		t.Fatalf("getChat of missing room = %s", got)
	}
}

func TestCheckKeyLen(t *testing.T) {
	for _, n := range []int{0, 32} {
		if err := checkKeyLen(n); err != nil {
			t.Errorf("checkKeyLen(%d) = %v", n, err)
		}
	}
	for _, n := range []int{-1, 1, 16, 31, 33} {
		if err := checkKeyLen(n); err == nil {
			t.Errorf("checkKeyLen(%d) accepted", n)
		}
	}
}

func TestHandleTableNeverReuses(t *testing.T) {
	first := insert(&entry{})
	second := insert(&entry{})
	if first < 1 || second != first+1 {
		t.Fatalf("handles = %d, %d", first, second)
	}
	if _, ok := remove(first); !ok {
		t.Fatal("remove of open handle failed")
	}
	if _, ok := lookup(first); ok {
		t.Fatal("removed handle still found")
	}
	if _, ok := remove(first); ok {
		t.Fatal("second remove reported success")
	}
	if third := insert(&entry{}); third != second+1 {
		t.Fatalf("handle %d reused after removal", third)
	}
	remove(second)
}
