package engine_test

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// TestLogoutErasesTheLoginsChatsAndMedia logs a demo account in, sends a picture, logs the account
// out, and checks that its chats, messages and picture file are gone and that it is no longer listed.
func TestLogoutErasesTheLoginsChatsAndMedia(t *testing.T) {
	dir := t.TempDir()
	c := startDemoCore(t, dir, &recorder{})
	t.Cleanup(c.Stop)
	loginDemo(t, c, "tester")
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
	if !ok {
		t.Fatal("DM chat with Alice missing")
	}
	pic, err := c.SendMedia(dm.RoomID, writePNG(t), "", "pic.png", "", "image", "", "txn-pic")
	if err != nil {
		t.Fatal(err)
	}
	var picURL string
	eventually(t, "picture sent with its media", func() bool {
		for _, m := range listMessages(t, c, dm.RoomID) {
			if m.EventID == pic && m.Status == "sent" && m.Media != nil {
				picURL = m.Media.URL
				return true
			}
		}
		return false
	})
	mediaFile := filepath.Join(dir, "media", picURL[strings.LastIndex(picURL, "/")+1:])
	if _, err := os.Stat(mediaFile); err != nil {
		t.Fatalf("picture file before logout: %v", err)
	}

	if err := c.Logout("demo", "demo-tester"); err != nil {
		t.Fatalf("Logout: %v", err)
	}
	if chats := listChats(t, c); len(chats) != 0 {
		t.Fatalf("chats left after logout: %+v", chats)
	}
	if msgs := listMessages(t, c, dm.RoomID); len(msgs) != 0 {
		t.Fatalf("messages left after logout: %d", len(msgs))
	}
	if _, err := os.Stat(mediaFile); !os.IsNotExist(err) {
		t.Fatalf("picture file survived logout: %v", err)
	}
	if strings.Contains(c.Accounts(), "demo-tester") {
		t.Fatalf("logged-out account still listed: %s", c.Accounts())
	}
}
