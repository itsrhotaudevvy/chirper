package engine_test

import (
	"strings"
	"testing"
	"time"

	"chirper.dev/core/facade"
)

// Seeded and echo bodies of the demo network, as the chat list shows them.
const (
	aliceNewestSeed = "Thanks!"
	teamNewestSeed  = "I'll send them tonight."
	echoBody        = "echo: ping"
)

// loginDemo logs in to the demo network as name and waits until both seeded chats are listed.
func loginDemo(t *testing.T, c *facade.Core, name string) {
	t.Helper()
	raw, err := c.StartLogin("demo", "demo")
	if err != nil {
		t.Fatal(err)
	}
	var state struct {
		ProcessID string `json:"processId"`
	}
	mustDecode(t, raw, &state)
	if _, err := c.SubmitLoginInput(state.ProcessID, `{"name":"`+name+`"}`); err != nil {
		t.Fatalf("login: %v", err)
	}
}

// loadAllHistory asks for older history until the network has no more, and reports how many
// LoadOlder calls returned more history.
func loadAllHistory(t *testing.T, c *facade.Core, roomID string) int {
	t.Helper()
	pages := 0
	for range 10 {
		more, err := c.LoadOlder(roomID)
		if err != nil {
			t.Fatalf("LoadOlder: %v", err)
		}
		if !more {
			return pages
		}
		pages++
	}
	t.Fatal("history never ended")
	return pages
}

// chatState is what the chat list shows for one chat: its preview and unread count.
type chatState struct {
	preview string
	unread  int
}

func demoChats(t *testing.T, c *facade.Core) map[string]chatState {
	t.Helper()
	var chats []chatJSON
	mustDecode(t, c.ListChats(""), &chats)
	out := map[string]chatState{}
	for _, chat := range chats {
		st := chatState{unread: chat.UnreadCount}
		if chat.Preview != nil {
			st.preview = chat.Preview.Body
		}
		out[chat.Name] = st
	}
	return out
}

// TestDemoHistoryIsStableAcrossRestarts restarts the core twice on the same data directory with
// the demo login. Backfill must not change the previews or grow the unread counts, and the
// message events must say which messages are live.
func TestDemoHistoryIsStableAcrossRestarts(t *testing.T) {
	dir := t.TempDir()
	rec := &recorder{}
	c := startDemoCore(t, dir, rec)
	t.Cleanup(c.Stop)
	loginDemo(t, c, "reader")
	eventually(t, "seeded chats", func() bool {
		chats := listChats(t, c)
		return len(chats) == 2 && len(listMessages(t, c, chats[0].RoomID)) >= 5 && len(listMessages(t, c, chats[1].RoomID)) >= 5
	})
	dm, _ := chatNamed(t, c, "Alice")
	team, _ := chatNamed(t, c, "Team")

	// Backfill both chats. The network has exactly three pages of history.
	for _, roomID := range []string{dm.RoomID, team.RoomID} {
		if pages := loadAllHistory(t, c, roomID); pages > 3 {
			t.Errorf("history of %s had %d pages, want at most 3", roomID, pages)
		}
	}
	if n := len(listMessages(t, c, dm.RoomID)); n != 5+60 {
		t.Fatalf("DM has %d messages after backfill, want 65", n)
	}
	eventually(t, "history delivered as not live", func() bool {
		return rec.seen("message", func(p string) bool {
			return strings.Contains(p, "history message") && strings.Contains(p, `"live":false`)
		})
	})

	// The seeded messages are old, so they are not live either.
	if rec.seen("message", func(p string) bool { return strings.Contains(p, aliceNewestSeed) && strings.Contains(p, `"live":true`) }) {
		t.Error("seeded message reported as live")
	}
	if got := demoChats(t, c); got["Alice"] != (chatState{preview: aliceNewestSeed}) || got["Team"] != (chatState{preview: teamNewestSeed}) {
		t.Fatalf("after backfill previews or unread wrong: %+v", got)
	}

	// A message from the user is answered by Alice a moment later. The echo is live and unread.
	if _, err := c.SendText(dm.RoomID, "ping", "", "txn-ping"); err != nil {
		t.Fatal(err)
	}
	eventually(t, "echo reply stored", func() bool {
		_, ok := findMessage(t, c, dm.RoomID, echoBody)
		return ok
	})
	eventually(t, "echo reply pushed as live", func() bool {
		return rec.seen("message", func(p string) bool {
			return strings.Contains(p, echoBody) && strings.Contains(p, `"live":true`)
		})
	})
	want := map[string]chatState{"Alice": {preview: echoBody, unread: 1}, "Team": {preview: teamNewestSeed}}
	eventually(t, "echo is the preview and the only unread message", func() bool {
		got := demoChats(t, c)
		return got["Alice"] == want["Alice"] && got["Team"] == want["Team"]
	})
	stored := len(listMessages(t, c, dm.RoomID))
	c.Stop()

	// Two restarts on the same data: the seeds are re-sent and the history is fetched again.
	for round := range 2 {
		rec := &recorder{}
		c := startDemoCore(t, dir, rec)
		t.Cleanup(c.Stop)
		eventually(t, "login restored", func() bool {
			var accounts []struct {
				LoginID string `json:"loginId"`
				State   string `json:"state"`
			}
			mustDecode(t, c.Accounts(), &accounts)
			for _, a := range accounts {
				if a.LoginID == "demo-reader" && a.State == "CONNECTED" {
					return true
				}
			}
			return false
		})
		dm, _ := chatNamed(t, c, "Alice")
		team, _ := chatNamed(t, c, "Team")
		loadAllHistory(t, c, dm.RoomID)
		loadAllHistory(t, c, team.RoomID)
		// Give the re-sent seeds and the backfill time to be applied before the check.
		time.Sleep(500 * time.Millisecond)
		if n := len(listMessages(t, c, dm.RoomID)); n != stored {
			t.Errorf("restart %d: DM has %d messages, want %d (history duplicated or lost)", round+1, n, stored)
		}
		got := demoChats(t, c)
		if got["Alice"] != want["Alice"] || got["Team"] != want["Team"] {
			t.Errorf("restart %d: previews or unread changed: %+v", round+1, got)
		}
		c.Stop()
	}
}
