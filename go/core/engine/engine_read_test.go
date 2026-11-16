package engine_test

import "testing"

// TestMarkUnreadAndLoadOlderOnTheDemoNetwork checks the read-state and history contract through the facade.
func TestMarkUnreadAndLoadOlderOnTheDemoNetwork(t *testing.T) {
	rec := &recorder{}
	c := startDemoCore(t, t.TempDir(), rec)
	t.Cleanup(c.Stop)
	first, err := c.StartLogin("demo", "demo")
	if err != nil {
		t.Fatal(err)
	}
	var state struct {
		ProcessID string `json:"processId"`
	}
	mustDecode(t, first, &state)
	if _, err := c.SubmitLoginInput(state.ProcessID, `{"name":"reader"}`); err != nil {
		t.Fatal(err)
	}
	eventually(t, "seeded chats", func() bool {
		chats := listChats(t, c)
		return len(chats) == 2 && len(listMessages(t, c, chats[0].RoomID)) >= 5
	})
	dm, ok := chatNamed(t, c, "Alice")
	if !ok {
		t.Fatal("DM chat with Alice missing")
	}
	if err := c.MarkUnread(dm.RoomID, true); err != nil {
		t.Fatal(err)
	}
	eventually(t, "chat marked unread", func() bool {
		chat, _ := chatNamed(t, c, "Alice")
		return chat.UnreadCount >= 1
	})
	if err := c.MarkUnread(dm.RoomID, false); err != nil {
		t.Fatal(err)
	}
	eventually(t, "chat marked read", func() bool {
		chat, _ := chatNamed(t, c, "Alice")
		return chat.UnreadCount == 0
	})
	if err := c.MarkUnread("!demo.none:chirper.local", true); err == nil {
		t.Error("marking an unknown chat unread succeeded")
	}
	if _, err := c.LoadOlder(dm.RoomID); err != nil {
		t.Errorf("LoadOlder: %v", err)
	}
}
