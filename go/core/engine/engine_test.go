package engine_test

import (
	"encoding/json"
	"image"
	"image/color"
	"image/png"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"chirper.dev/core/facade"
)

// recorder is the EventListener of the test core. It keeps every event in order.
type recorder struct {
	mu     sync.Mutex
	events []recorded
}

type recorded struct {
	kind    string
	payload string
}

func (r *recorder) OnEvent(kind, payload string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.events = append(r.events, recorded{kind: kind, payload: payload})
}

// seen reports whether an event of kind whose payload satisfies match was delivered.
func (r *recorder) seen(kind string, match func(payload string) bool) bool {
	r.mu.Lock()
	defer r.mu.Unlock()
	for _, ev := range r.events {
		if ev.kind == kind && match(ev.payload) {
			return true
		}
	}
	return false
}

type chatJSON struct {
	RoomID  string   `json:"roomId"`
	Network string   `json:"network"`
	LoginID string   `json:"loginId"`
	Name    string   `json:"name"`
	IsDM    bool     `json:"isDm"`
	IsGroup bool     `json:"isGroup"`
	Typing  []string `json:"typing"`

	UnreadCount int `json:"unreadCount"`
	Preview     *struct {
		Body string `json:"body"`
	} `json:"preview"`
}

type reactionJSON struct {
	Key     string   `json:"key"`
	Count   int      `json:"count"`
	FromMe  bool     `json:"fromMe"`
	Senders []string `json:"senders"`
}

type messageJSON struct {
	EventID    string         `json:"eventId"`
	RoomID     string         `json:"roomId"`
	Sender     string         `json:"sender"`
	SenderName string         `json:"senderName"`
	FromMe     bool           `json:"fromMe"`
	Type       string         `json:"type"`
	Body       string         `json:"body"`
	Media      *mediaJSON     `json:"media"`
	Edited     bool           `json:"edited"`
	Redacted   bool           `json:"redacted"`
	Reactions  []reactionJSON `json:"reactions"`
	Status     string         `json:"status"`
	Live       bool           `json:"live"`
}

type mediaJSON struct {
	URL   string `json:"url"`
	Mime  string `json:"mime"`
	Width int    `json:"width"`
}

// eventually polls cond until it holds or the deadline passes.
func eventually(t *testing.T, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(20 * time.Second)
	for time.Now().Before(deadline) {
		if cond() {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatalf("timed out waiting for %s", what)
}

func mustDecode(t *testing.T, raw string, v any) {
	t.Helper()
	if err := json.Unmarshal([]byte(raw), v); err != nil {
		t.Fatalf("invalid JSON %q: %v", raw, err)
	}
}

func listChats(t *testing.T, c *facade.Core) []chatJSON {
	t.Helper()
	var chats []chatJSON
	mustDecode(t, c.ListChats(""), &chats)
	return chats
}

func chatNamed(t *testing.T, c *facade.Core, name string) (chatJSON, bool) {
	t.Helper()
	for _, chat := range listChats(t, c) {
		if chat.Name == name {
			return chat, true
		}
	}
	return chatJSON{}, false
}

func listMessages(t *testing.T, c *facade.Core, roomID string) []messageJSON {
	t.Helper()
	var msgs []messageJSON
	mustDecode(t, c.ListMessages(roomID, 0, 200), &msgs)
	return msgs
}

// findMessage returns the message of roomID whose body is body.
func findMessage(t *testing.T, c *facade.Core, roomID, body string) (messageJSON, bool) {
	t.Helper()
	for _, m := range listMessages(t, c, roomID) {
		if m.Body == body {
			return m, true
		}
	}
	return messageJSON{}, false
}

// startDemoCore opens a core under dir with the demo network enabled and starts it.
func startDemoCore(t *testing.T, dir string, rec *recorder) *facade.Core {
	t.Helper()
	c, err := facade.NewCore(dir, rec)
	if err != nil {
		t.Fatal(err)
	}
	if err := c.SetSettings(`{"demoNetwork":true,"sendReadReceipts":{"*":true},"sendTyping":{"*":true}}`); err != nil {
		t.Fatal(err)
	}
	if err := c.Start(); err != nil {
		t.Fatal(err)
	}
	return c
}

// writePNG writes a 3x2 PNG and returns its path.
func writePNG(t *testing.T) string {
	t.Helper()
	img := image.NewRGBA(image.Rect(0, 0, 3, 2))
	for x := 0; x < 3; x++ {
		img.Set(x, 0, color.RGBA{R: 255, A: 255})
	}
	path := filepath.Join(t.TempDir(), "pic.png")
	f, err := os.Create(path)
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	if err := png.Encode(f, img); err != nil {
		t.Fatal(err)
	}
	return path
}

func TestDemoNetworkThroughMobileFacade(t *testing.T) {
	dir := t.TempDir()
	rec := &recorder{}
	c := startDemoCore(t, dir, rec)
	t.Cleanup(c.Stop)

	var networks []struct {
		ID         string `json:"id"`
		LoginFlows []struct {
			ID string `json:"id"`
		} `json:"loginFlows"`
	}
	mustDecode(t, c.Networks(), &networks)
	demoListed := false
	for _, n := range networks {
		if n.ID == "demo" {
			demoListed = len(n.LoginFlows) == 1 && n.LoginFlows[0].ID == "demo"
		}
	}
	if !demoListed {
		t.Fatalf("demo network with its login flow not listed: %+v", networks)
	}

	// Log in: the demo flow asks for an account name.
	var first struct {
		ProcessID string `json:"processId"`
		Step      struct {
			Type      string `json:"type"`
			UserInput struct {
				Fields []struct {
					ID string `json:"id"`
				} `json:"fields"`
			} `json:"userInput"`
		} `json:"step"`
	}
	raw, err := c.StartLogin("demo", "demo")
	if err != nil {
		t.Fatal(err)
	}
	mustDecode(t, raw, &first)
	if first.Step.Type != "user_input" || first.Step.UserInput.Fields[0].ID != "name" {
		t.Fatalf("first login step = %s", raw)
	}
	raw, err = c.SubmitLoginInput(first.ProcessID, `{"name":"tester"}`)
	if err != nil {
		t.Fatalf("login: %v", err)
	}
	var done struct {
		Step struct {
			Type     string `json:"type"`
			Complete struct {
				LoginID string `json:"loginId"`
			} `json:"complete"`
		} `json:"step"`
	}
	mustDecode(t, raw, &done)
	if done.Step.Type != "complete" || done.Step.Complete.LoginID != "demo-tester" {
		t.Fatalf("login did not complete: %s", raw)
	}

	// The seeded chats arrive through the bridge and land in the store.
	eventually(t, "two seeded chats with five messages each", func() bool {
		chats := listChats(t, c)
		if len(chats) != 2 {
			return false
		}
		for _, chat := range chats {
			if len(listMessages(t, c, chat.RoomID)) < 5 {
				return false
			}
		}
		return true
	})
	dm, ok := chatNamed(t, c, "Alice")
	if !ok || !dm.IsDM || dm.LoginID != "demo-tester" {
		t.Fatalf("DM chat with Alice missing, not a DM, or without its login: %+v", dm)
	}
	team, ok := chatNamed(t, c, "Team")
	if !ok || !team.IsGroup {
		t.Fatalf("group chat Team missing or wrong kind: %+v", team)
	}
	if _, ok := findMessage(t, c, dm.RoomID, "Hi! This is a demo chat."); !ok {
		t.Error("seeded DM message missing")
	}
	teamMsgs := listMessages(t, c, team.RoomID)
	if len(teamMsgs) != 5 || teamMsgs[0].SenderName != "Bob" || !teamMsgs[2].FromMe {
		t.Errorf("team history wrong: %+v", teamMsgs)
	}

	// Send a message: it is stored as sending, the network confirms it, and the contact answers.
	eventID, err := c.SendText(dm.RoomID, "hello from the UI", "", "txn-hello")
	if err != nil {
		t.Fatal(err)
	}
	eventually(t, "outgoing message marked sent", func() bool {
		m, ok := findMessage(t, c, dm.RoomID, "hello from the UI")
		return ok && m.Status == "sent" && m.EventID == eventID
	})
	eventually(t, "echo reply from Alice", func() bool {
		_, ok := findMessage(t, c, dm.RoomID, "echo: hello from the UI")
		return ok
	})
	eventually(t, "Alice's reaction on the outgoing message", func() bool {
		m, ok := findMessage(t, c, dm.RoomID, "hello from the UI")
		if !ok || len(m.Reactions) == 0 {
			return false
		}
		// Bridged emoji keys carry the U+FE0F variation selector that bridgev2 adds.
		key := strings.ReplaceAll(m.Reactions[0].Key, "️", "")
		return key == "👍" && m.Reactions[0].Count == 1 && strings.Contains(strings.Join(m.Reactions[0].Senders, ","), "Alice")
	})

	// A duplicate transaction is refused rather than sent twice.
	if _, err := c.SendText(dm.RoomID, "again", "", "txn-hello"); err == nil {
		t.Error("duplicate transaction was accepted")
	}

	// Edit the outgoing message. The store shows the edit at once and the network confirms it.
	if err := c.Edit(dm.RoomID, eventID, "hello, edited"); err != nil {
		t.Fatal(err)
	}
	eventually(t, "edit applied to the stored message", func() bool {
		m, ok := findMessage(t, c, dm.RoomID, "hello, edited")
		return ok && m.Edited && m.EventID == eventID
	})

	// React, then remove the reaction again.
	if err := c.React(dm.RoomID, eventID, "❤"); err != nil {
		t.Fatal(err)
	}
	eventually(t, "own reaction shown", func() bool {
		m, ok := findMessage(t, c, dm.RoomID, "hello, edited")
		return ok && hasReaction(m, "❤", true)
	})
	if err := c.Unreact(dm.RoomID, eventID, "❤"); err != nil {
		t.Fatal(err)
	}
	eventually(t, "own reaction removed", func() bool {
		m, ok := findMessage(t, c, dm.RoomID, "hello, edited")
		return ok && !hasReaction(m, "❤", true)
	})

	// Send a picture: it is uploaded into the local media store and can be read back by URI.
	pic, err := c.SendMedia(dm.RoomID, writePNG(t), "", "pic.png", "", "image", "", "txn-pic")
	if err != nil {
		t.Fatal(err)
	}
	eventually(t, "picture sent with its media", func() bool {
		for _, m := range listMessages(t, c, dm.RoomID) {
			if m.EventID == pic && m.Type == "image" && m.Status == "sent" && m.Media != nil && m.Media.Width == 3 {
				return m.Media.Mime == "image/png" && strings.HasPrefix(m.Media.URL, "mxc://chirper.local/")
			}
		}
		return false
	})
	var mediaURL string
	for _, m := range listMessages(t, c, dm.RoomID) {
		if m.EventID == pic {
			mediaURL = m.Media.URL
		}
	}
	path, err := c.MediaFile(mediaURL)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(path); err != nil {
		t.Errorf("media file missing: %v", err)
	}

	// Redact the picture. The store clears it at once and the network confirms the removal.
	if err := c.Redact(dm.RoomID, pic); err != nil {
		t.Fatal(err)
	}
	eventually(t, "picture redacted", func() bool {
		for _, m := range listMessages(t, c, dm.RoomID) {
			if m.EventID == pic {
				return m.Redacted
			}
		}
		return false
	})

	// Local-only operations succeed.
	if err := c.MarkRead(dm.RoomID, eventID); err != nil {
		t.Fatal(err)
	}
	if err := c.SetTyping(dm.RoomID, true); err != nil {
		t.Fatal(err)
	}
	if err := c.SetChatLocalFlags(dm.RoomID, `{"pinned":true}`); err != nil {
		t.Fatal(err)
	}
	if chat, _ := chatNamed(t, c, "Alice"); !chat.IsDM {
		t.Error("chat lost its kind after local flags")
	}

	// The account is connected and the events reached the listener.
	eventually(t, "account connected", func() bool {
		var accounts []struct {
			Network string `json:"network"`
			LoginID string `json:"loginId"`
			State   string `json:"state"`
		}
		mustDecode(t, c.Accounts(), &accounts)
		for _, a := range accounts {
			if a.Network == "demo" && a.LoginID == "demo-tester" && a.State == "CONNECTED" {
				return true
			}
		}
		return false
	})
	// Events are delivered asynchronously, so each one is awaited.
	for _, kind := range []string{"chat", "message", "account"} {
		eventually(t, kind+" event delivered", func() bool {
			return rec.seen(kind, func(string) bool { return true })
		})
	}
	eventually(t, "echo reply pushed as a message event", func() bool {
		return rec.seen("message", func(p string) bool { return strings.Contains(p, "echo: hello from the UI") })
	})
	eventually(t, "connected account pushed as an account event", func() bool {
		return rec.seen("account", func(p string) bool { return strings.Contains(p, `"state":"CONNECTED"`) })
	})

	c.Stop()

	// A restart restores the login and the history from the same databases.
	rec2 := &recorder{}
	c2 := startDemoCore(t, dir, rec2)
	t.Cleanup(c2.Stop)
	eventually(t, "login restored after restart", func() bool {
		var accounts []struct {
			LoginID string `json:"loginId"`
			State   string `json:"state"`
		}
		mustDecode(t, c2.Accounts(), &accounts)
		for _, a := range accounts {
			if a.LoginID == "demo-tester" && a.State == "CONNECTED" {
				return true
			}
		}
		return false
	})
	restored, ok := chatNamed(t, c2, "Alice")
	if !ok {
		t.Fatal("DM chat missing after restart")
	}
	if _, ok := findMessage(t, c2, restored.RoomID, "hello, edited"); !ok {
		t.Error("edited message missing after restart")
	}
}

// hasReaction reports whether m has reaction key, from the user when fromMe is set.
func hasReaction(m messageJSON, key string, fromMe bool) bool {
	for _, r := range m.Reactions {
		if strings.ReplaceAll(r.Key, "️", "") == key && r.FromMe == fromMe && r.Count > 0 {
			return true
		}
	}
	return false
}
