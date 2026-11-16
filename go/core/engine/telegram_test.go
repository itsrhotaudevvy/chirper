package engine

import (
	"strings"
	"testing"
)

type nopListener struct{}

func (nopListener) OnEvent(string, string) {}

// networkByID returns the listed network row with id.
func networkByID(t *testing.T, e *Engine, id string) Network {
	t.Helper()
	for _, n := range e.Networks() {
		if n.ID == id {
			return n
		}
	}
	t.Fatalf("network %s is not listed", id)
	return Network{}
}

func TestTelegramIsListedWithItsCredentialStatus(t *testing.T) {
	e, err := New(t.TempDir(), nopListener{})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(e.Stop)

	tg := networkByID(t, e, "telegram")
	if tg.Available || !strings.Contains(tg.Reason, "api_id and api_hash") || tg.Name != "Telegram" {
		t.Fatalf("telegram without credentials = %+v", tg)
	}
	if len(tg.LoginFlows) != 0 {
		t.Errorf("unavailable telegram lists login flows: %+v", tg.LoginFlows)
	}

	if err := e.SetSettings(Settings{
		SendReadReceipts: map[string]bool{"*": true},
		SendTyping:       map[string]bool{"*": true},
		Telegram:         TelegramAPI{APIID: 12345, APIHash: "0123456789abcdef"},
	}); err != nil {
		t.Fatal(err)
	}
	tg = networkByID(t, e, "telegram")
	if !tg.Available || tg.Reason != "" || len(tg.LoginFlows) == 0 {
		t.Fatalf("telegram with credentials = %+v", tg)
	}
}

func TestTelegramCredentialsFallBackToBuildDefaults(t *testing.T) {
	oldID, oldHash := telegramDefaultAPIID, telegramDefaultAPIHash
	t.Cleanup(func() { telegramDefaultAPIID, telegramDefaultAPIHash = oldID, oldHash })
	telegramDefaultAPIID, telegramDefaultAPIHash = "777", "buildhash"

	if id, hash := telegramCredentials(Settings{}); id != 777 || hash != "buildhash" {
		t.Errorf("defaults = %d %q", id, hash)
	}
	s := Settings{Telegram: TelegramAPI{APIID: 42, APIHash: "mine"}}
	if id, hash := telegramCredentials(s); id != 42 || hash != "mine" {
		t.Errorf("settings must win over defaults, got %d %q", id, hash)
	}
	telegramDefaultAPIID = "not a number"
	if id, _ := telegramCredentials(Settings{}); id != 0 {
		t.Errorf("invalid build default id = %d, want 0", id)
	}
}

func TestSettingsCarryTelegramCredentialsInContractShape(t *testing.T) {
	s := defaultSettings()
	s.Telegram = TelegramAPI{APIID: 9, APIHash: "h"}
	if got := mustJSON(s); !strings.Contains(got, `"telegram":{"apiId":9,"apiHash":"h"}`) {
		t.Errorf("settings JSON = %s", got)
	}
}

func TestUnavailableRowsKeepTheirReason(t *testing.T) {
	spec := networkSpec{id: "x", unavailable: "not here"}
	if got := spec.unavailableReason(Settings{}); got != "not here" {
		t.Errorf("reason = %q", got)
	}
	if got := (networkSpec{id: "y"}).unavailableReason(Settings{}); got != "" {
		t.Errorf("available spec has reason %q", got)
	}
}
