package store

import (
	"context"
	"database/sql"
	"strings"
	"testing"

	"maunium.net/go/mautrix/event"
)

func pragmaValue(t *testing.T, s *Store, name string) string {
	t.Helper()
	var v string
	row := s.db.QueryRow(context.Background(), "PRAGMA "+name)
	if err := row.Scan(&v); err != nil && err != sql.ErrNoRows {
		t.Fatalf("PRAGMA %s: %v", name, err)
	}
	return v
}

func TestConnectionPragmas(t *testing.T) {
	s := openTest(t)
	want := map[string]string{
		"foreign_keys":  "1",
		"journal_mode":  "wal",
		"synchronous":   "1", // NORMAL
		"busy_timeout":  "5000",
		"temp_store":    "2", // MEMORY
		"cache_size":    "-16000",
		"secure_delete": "1",
	}
	for name, value := range want {
		if got := pragmaValue(t, s, name); got != value {
			t.Errorf("PRAGMA %s = %q, want %q", name, got, value)
		}
	}
}

func TestHotQueryIndicesExist(t *testing.T) {
	s := openTest(t)
	for _, name := range []string{"event_room_ord_idx", "room_list_idx", "reaction_target_idx"} {
		var n int
		row := s.db.QueryRow(context.Background(), `SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = $1`, name)
		if err := row.Scan(&n); err != nil || n != 1 {
			t.Errorf("index %s missing (count %d, err %v)", name, n, err)
		}
	}
}

func TestDeleteRoomErasesContent(t *testing.T) {
	ctx := context.Background()
	s := openTest(t)
	if err := s.SetRoomFields(ctx, testRoom, RoomFields{Name: strPtr("Secret"), Topic: strPtr("plans")}); err != nil {
		t.Fatal(err)
	}
	mustInsert(t, s, textEvent("secret", "alice", 10, "the launch code is 1234"))
	if _, err := s.AddReaction(ctx, &Reaction{EventID: id2("r"), RoomID: testRoom,
		TargetEventID: id2("secret"), Sender: "@alice:chirper.local", Key: "+1", TS: 11}); err != nil {
		t.Fatal(err)
	}
	if err := s.SetMembership(ctx, testRoom, mustUser("alice"), event.MembershipJoin); err != nil {
		t.Fatal(err)
	}

	if err := s.DeleteRoom(ctx, testRoom); err != nil {
		t.Fatalf("DeleteRoom: %v", err)
	}
	for _, query := range []string{
		"SELECT COUNT(*) FROM event WHERE room_id = $1",
		"SELECT COUNT(*) FROM reaction WHERE room_id = $1",
		"SELECT COUNT(*) FROM member WHERE room_id = $1",
	} {
		var n int
		if err := s.db.QueryRow(ctx, query, testRoom).Scan(&n); err != nil || n != 0 {
			t.Errorf("%s: count %d, err %v", query, n, err)
		}
	}
	room, err := s.GetRoom(ctx, testRoom)
	if err != nil {
		t.Fatal(err)
	}
	if !room.Deleted || room.Name != "" || room.Topic != "" || room.LastEventID != "" {
		t.Errorf("room content not cleared: %+v", room)
	}
	if _, err := s.GetEvent(ctx, id2("secret")); err != ErrNotFound {
		t.Errorf("deleted message still readable: %v", err)
	}
}

func TestRedactClearsContentAndErrorText(t *testing.T) {
	ctx := context.Background()
	s := openTest(t)
	e := textEvent("oops", "alice", 20, "private text")
	e.Error = "send failed: private text"
	mustInsert(t, s, e)
	if _, err := s.Redact(ctx, id2("oops")); err != nil {
		t.Fatal(err)
	}
	stored, err := s.GetEvent(ctx, id2("oops"))
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(stored.ContentJSON, "private") || strings.Contains(stored.Error, "private") {
		t.Fatalf("redacted event still holds text: content %q error %q", stored.ContentJSON, stored.Error)
	}
	if stored.ContentJSON != "{}" || !stored.Redacted {
		t.Errorf("redaction incomplete: %+v", stored)
	}
}
