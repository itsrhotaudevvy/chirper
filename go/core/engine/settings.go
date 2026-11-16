package engine

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
)

// Settings are the user settings stored in settings.json. Per-network maps use "*" as the default.
type Settings struct {
	DemoNetwork      bool            `json:"demoNetwork"`
	SendReadReceipts map[string]bool `json:"sendReadReceipts"`
	SendTyping       map[string]bool `json:"sendTyping"`
	Telegram         TelegramAPI     `json:"telegram"`
}

// TelegramAPI holds the api_id and api_hash that Telegram requires from my.telegram.org. Zero values
// fall back to the build defaults (see telegramCredentials).
type TelegramAPI struct {
	APIID   int    `json:"apiId"`
	APIHash string `json:"apiHash"`
}

func defaultSettings() Settings {
	return Settings{
		SendReadReceipts: map[string]bool{"*": true},
		SendTyping:       map[string]bool{"*": true},
	}
}

// allows reports whether the per-network flag m is set for network.
func allows(m map[string]bool, network string) bool {
	if v, ok := m[network]; ok {
		return v
	}
	if v, ok := m["*"]; ok {
		return v
	}
	return true
}

func loadSettings(path string) (Settings, error) {
	s := defaultSettings()
	raw, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return s, nil
	} else if err != nil {
		return s, fmt.Errorf("failed to read settings: %w", err)
	}
	if err := json.Unmarshal(raw, &s); err != nil {
		return s, fmt.Errorf("failed to parse settings: %w", err)
	}
	return s, nil
}

// saveSettings writes the settings atomically with owner-only permissions.
func saveSettings(path string, s Settings) error {
	raw, err := json.MarshalIndent(s, "", "  ")
	if err != nil {
		return fmt.Errorf("failed to encode settings: %w", err)
	}
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, raw, 0o600); err != nil {
		return fmt.Errorf("failed to write settings: %w", err)
	}
	if err := os.Rename(tmp, path); err != nil {
		return fmt.Errorf("failed to replace settings: %w", err)
	}
	return nil
}

// currentSettings returns a copy of the settings.
func (e *Engine) currentSettings() Settings {
	e.settingsMu.Lock()
	defer e.settingsMu.Unlock()
	return copySettings(e.settings)
}

// Settings returns the current settings.
func (e *Engine) Settings() Settings {
	return e.currentSettings()
}

// SetSettings validates and stores the settings. A change of DemoNetwork applies at the next Start.
func (e *Engine) SetSettings(s Settings) error {
	if s.SendReadReceipts == nil {
		s.SendReadReceipts = defaultSettings().SendReadReceipts
	}
	if s.SendTyping == nil {
		s.SendTyping = defaultSettings().SendTyping
	}
	e.settingsMu.Lock()
	defer e.settingsMu.Unlock()
	if err := saveSettings(e.settingsPath, s); err != nil {
		return err
	}
	e.settings = copySettings(s)
	return nil
}

func copySettings(s Settings) Settings {
	out := s
	out.SendReadReceipts = copyFlags(s.SendReadReceipts)
	out.SendTyping = copyFlags(s.SendTyping)
	return out
}

func copyFlags(m map[string]bool) map[string]bool {
	out := make(map[string]bool, len(m))
	for k, v := range m {
		out[k] = v
	}
	return out
}
