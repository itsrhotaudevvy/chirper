package store

import (
	"context"
	"encoding/json"
	"fmt"
	"path/filepath"
	"testing"

	"maunium.net/go/mautrix/event"
	"maunium.net/go/mautrix/id"
)

// benchFixture holds a store with 200 rooms and 5000 messages (25 per room), each room with
// three joined members, a reaction on every fifth message, and one long room for paging.
type benchFixture struct {
	store *Store
	long  id.RoomID
}

func newBenchFixture(b *testing.B) *benchFixture {
	b.Helper()
	ctx := context.Background()
	s, err := Open(ctx, filepath.Join(b.TempDir(), "chirper.db"))
	if err != nil {
		b.Fatalf("Open: %v", err)
	}
	b.Cleanup(func() { _ = s.Close() })
	f := &benchFixture{store: s, long: id.RoomID("!long:chirper.local")}
	seq := int64(1)
	addMessage := func(room id.RoomID, n int) {
		content, _ := json.Marshal(event.MessageEventContent{MsgType: event.MsgText, Body: fmt.Sprintf("message %d in %s", n, room)})
		e := &Event{
			EventID:     id.EventID(fmt.Sprintf("$%s-%d", room, n)),
			RoomID:      room,
			Sender:      id.UserID(fmt.Sprintf("@user%d:chirper.local", n%3)),
			Type:        typeMessage,
			ContentJSON: string(content),
			TS:          seq,
		}
		seq++
		if _, err := s.InsertEvent(ctx, e); err != nil {
			b.Fatalf("InsertEvent: %v", err)
		}
		if n%5 == 0 {
			r := &Reaction{EventID: id.EventID(fmt.Sprintf("$r-%s-%d", room, n)), RoomID: room,
				TargetEventID: e.EventID, Sender: "@user1:chirper.local", Key: "+1", TS: seq}
			if _, err := s.AddReaction(ctx, r); err != nil {
				b.Fatalf("AddReaction: %v", err)
			}
		}
	}
	for r := 0; r < 200; r++ {
		room := id.RoomID(fmt.Sprintf("!room%03d:chirper.local", r))
		if err := s.UpsertRoom(ctx, &Room{RoomID: room, BridgeID: "demo", CreatedTS: 1}); err != nil {
			b.Fatalf("UpsertRoom: %v", err)
		}
		if err := s.SetRoomFields(ctx, room, RoomFields{Name: strPtr(fmt.Sprintf("Chat %03d", r))}); err != nil {
			b.Fatal(err)
		}
		for u := 0; u < 3; u++ {
			if err := s.UpsertMember(ctx, &Member{RoomID: room, UserID: id.UserID(fmt.Sprintf("@user%d:chirper.local", u)),
				Displayname: fmt.Sprintf("User %d", u), Membership: event.MembershipJoin}); err != nil {
				b.Fatal(err)
			}
		}
		for n := 0; n < 25; n++ {
			addMessage(room, n)
		}
	}
	if err := s.UpsertRoom(ctx, &Room{RoomID: f.long, BridgeID: "demo", CreatedTS: 1}); err != nil {
		b.Fatal(err)
	}
	for n := 0; n < 5000; n++ {
		addMessage(f.long, n)
	}
	return f
}

// BenchmarkListChats lists the unarchived chats of the 200-room fixture, with previews.
func BenchmarkListChats(b *testing.B) {
	f := newBenchFixture(b)
	ctx := context.Background()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		chats, err := f.store.ListChats(ctx, ChatFilter{})
		if err != nil {
			b.Fatal(err)
		}
		if len(chats) != 201 {
			b.Fatalf("got %d chats", len(chats))
		}
	}
}

// BenchmarkListMessagesPage pages backwards through the 5000-message room, 50 messages per call.
func BenchmarkListMessagesPage(b *testing.B) {
	f := newBenchFixture(b)
	ctx := context.Background()
	var before int64
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		msgs, err := f.store.ListMessages(ctx, f.long, before, 50)
		if err != nil {
			b.Fatal(err)
		}
		if len(msgs) == 0 {
			before = 0
			continue
		}
		before = msgs[0].Order
	}
}
