package store

import (
	"context"
	"encoding/json"
	"fmt"
	"path/filepath"
	"strings"
	"testing"

	"maunium.net/go/mautrix/event"
	"maunium.net/go/mautrix/id"
)

const testRoom id.RoomID = "!demo.test:chirper.local"

func openTest(t *testing.T) *Store {
	t.Helper()
	s, err := Open(context.Background(), filepath.Join(t.TempDir(), "chirper.db"))
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	t.Cleanup(func() { _ = s.Close() })
	ctx := context.Background()
	if err := s.UpsertRoom(ctx, &Room{RoomID: testRoom, BridgeID: "demo", CreatedTS: 1}); err != nil {
		t.Fatalf("UpsertRoom: %v", err)
	}
	return s
}

// textEvent builds a message event with a plain text body.
func textEvent(id, sender string, ts int64, body string) *Event {
	content, _ := json.Marshal(event.MessageEventContent{MsgType: event.MsgText, Body: body})
	return &Event{
		EventID:     id2(id),
		RoomID:      testRoom,
		Sender:      mustUser(sender),
		Type:        typeMessage,
		ContentJSON: string(content),
		TS:          ts,
		Status:      "",
	}
}

func id2(s string) id.EventID { return id.EventID("$" + s) }

func mustUser(localpart string) id.UserID {
	if strings.HasPrefix(localpart, "@") {
		return id.UserID(localpart)
	}
	return id.UserID("@" + localpart + ":chirper.local")
}

func mustInsert(t *testing.T, s *Store, e *Event) {
	t.Helper()
	inserted, err := s.InsertEvent(context.Background(), e)
	if err != nil {
		t.Fatalf("InsertEvent(%s): %v", e.EventID, err)
	}
	if !inserted {
		t.Fatalf("InsertEvent(%s): expected insert", e.EventID)
	}
}

func TestInsertEventIsIdempotentAndOrdersByTimestamp(t *testing.T) {
	ctx := context.Background()
	s := openTest(t)

	a := textEvent("a", "alice", 1000, "first")
	b := textEvent("b", "alice", 1000, "same ms")
	c := textEvent("c", "alice", 999, "older")
	mustInsert(t, s, a)
	mustInsert(t, s, b)
	mustInsert(t, s, c)

	if b.Order != a.Order+1 {
		t.Errorf("same-ms events should get consecutive orders: a=%d b=%d", a.Order, b.Order)
	}
	if c.Order >= a.Order {
		t.Errorf("older event should sort before: c=%d a=%d", c.Order, a.Order)
	}

	dup := textEvent("a", "alice", 5000, "replayed")
	inserted, err := s.InsertEvent(ctx, dup)
	if err != nil {
		t.Fatalf("InsertEvent duplicate: %v", err)
	}
	if inserted {
		t.Error("duplicate event ID must not be inserted again")
	}
	if dup.Order != a.Order {
		t.Errorf("duplicate should report the stored order: got %d want %d", dup.Order, a.Order)
	}

	msgs, err := s.ListMessages(ctx, testRoom, 0, 10)
	if err != nil {
		t.Fatalf("ListMessages: %v", err)
	}
	if len(msgs) != 3 {
		t.Fatalf("want 3 messages, got %d", len(msgs))
	}
	if msgs[0].Body != "older" || msgs[2].Body != "same ms" {
		t.Errorf("messages not newest-last: %q ... %q", msgs[0].Body, msgs[2].Body)
	}
}

func TestListMessagesPagination(t *testing.T) {
	ctx := context.Background()
	s := openTest(t)
	for i := range 5 {
		mustInsert(t, s, textEvent(fmt.Sprintf("m%d", i), "alice", int64(100+i), fmt.Sprintf("body %d", i)))
	}
	latest, err := s.ListMessages(ctx, testRoom, 0, 2)
	if err != nil {
		t.Fatal(err)
	}
	if len(latest) != 2 || latest[0].Body != "body 3" || latest[1].Body != "body 4" {
		t.Fatalf("latest page wrong: %+v", latest)
	}
	older, err := s.ListMessages(ctx, testRoom, latest[0].Order, 10)
	if err != nil {
		t.Fatal(err)
	}
	if len(older) != 3 || older[2].Body != "body 2" {
		t.Fatalf("older page wrong: %+v", older)
	}
}

func TestEditAndRedact(t *testing.T) {
	ctx := context.Background()
	s := openTest(t)
	mustInsert(t, s, textEvent("orig", "alice", 100, "hello"))

	if err := s.ApplyEdit(ctx, testRoom, id2("orig"), `{"msgtype":"m.text","body":"hello, edited"}`, 200); err != nil {
		t.Fatalf("ApplyEdit: %v", err)
	}
	msg, err := s.MessageByID(ctx, id2("orig"))
	if err != nil {
		t.Fatal(err)
	}
	if msg.Body != "hello, edited" || !msg.Edited {
		t.Errorf("edit not applied: body=%q edited=%v", msg.Body, msg.Edited)
	}

	red, err := s.Redact(ctx, id2("orig"))
	if err != nil {
		t.Fatalf("Redact: %v", err)
	}
	if red.IsReaction || red.TargetEventID != id2("orig") || red.RoomID != testRoom {
		t.Errorf("unexpected redaction result: %+v", red)
	}
	msg, err = s.MessageByID(ctx, id2("orig"))
	if err != nil {
		t.Fatal(err)
	}
	if !msg.Redacted || msg.Body != "" || msg.Edited {
		t.Errorf("redacted message keeps content: %+v", msg)
	}

	if _, err := s.Redact(ctx, id2("missing")); err != ErrNotFound {
		t.Errorf("redacting unknown event: want ErrNotFound, got %v", err)
	}
	if err := s.ApplyEdit(ctx, testRoom, id2("orig"), `{}`, 300); err != ErrNotFound {
		t.Errorf("editing redacted event: want ErrNotFound, got %v", err)
	}
}

func TestReactionsAggregateAndRedact(t *testing.T) {
	ctx := context.Background()
	s := openTest(t)
	mustInsert(t, s, textEvent("msg", SelfUserID.Localpart(), 100, "mine"))
	mustInsert(t, s, textEvent("reply", "alice", 110, "hi"))

	add := func(eventID, sender, key string, ts int64) bool {
		t.Helper()
		ok, err := s.AddReaction(ctx, &Reaction{EventID: id2(eventID), RoomID: testRoom,
			TargetEventID: id2("msg"), Sender: mustUser(sender), Key: key, TS: ts})
		if err != nil {
			t.Fatalf("AddReaction: %v", err)
		}
		return ok
	}
	add("r1", "alice", "👍", 120)
	if add("r1b", "alice", "👍", 121) {
		t.Error("same sender, same key, same target must not be stored twice")
	}
	add("r2", SelfUserID.Localpart(), "👍", 122)
	add("r3", "alice", "❤", 123)

	msg, err := s.MessageByID(ctx, id2("msg"))
	if err != nil {
		t.Fatal(err)
	}
	if len(msg.Reactions) != 2 {
		t.Fatalf("want 2 reaction keys, got %+v", msg.Reactions)
	}
	thumbs := msg.Reactions[0]
	if thumbs.Key != "👍" || thumbs.Count != 2 || !thumbs.FromMe {
		t.Errorf("thumbs aggregate wrong: %+v", thumbs)
	}
	if strings.Join(thumbs.Senders, ",") != "Alice,me" && strings.Join(thumbs.Senders, ",") != "@alice:chirper.local,me" {
		t.Errorf("unexpected senders: %v", thumbs.Senders)
	}

	red, err := s.Redact(ctx, id2("r1"))
	if err != nil {
		t.Fatalf("Redact reaction: %v", err)
	}
	if !red.IsReaction || red.TargetEventID != id2("msg") {
		t.Errorf("reaction redaction result wrong: %+v", red)
	}
	msg, _ = s.MessageByID(ctx, id2("msg"))
	if msg.Reactions[0].Count != 1 || !msg.Reactions[0].FromMe {
		t.Errorf("reaction not removed: %+v", msg.Reactions)
	}
}

func TestUnreadCountAndReadMarker(t *testing.T) {
	ctx := context.Background()
	s := openTest(t)
	for i, sender := range []string{"alice", "alice", SelfUserID.Localpart(), "bob"} {
		e := textEvent(fmt.Sprintf("u%d", i), sender, int64(100+i), "x")
		mustInsert(t, s, e)
		if e.Sender != SelfUserID {
			if err := s.IncrementUnread(ctx, e.EventID); err != nil {
				t.Fatal(err)
			}
		}
	}
	room, _ := s.GetRoom(ctx, testRoom)
	if room.UnreadCount != 3 {
		t.Fatalf("unread after 3 incoming: got %d", room.UnreadCount)
	}
	if err := s.MarkReadUpTo(ctx, testRoom, id2("u1")); err != nil {
		t.Fatal(err)
	}
	room, _ = s.GetRoom(ctx, testRoom)
	if room.UnreadCount != 1 {
		t.Errorf("unread after reading u1: got %d want 1 (only bob's message)", room.UnreadCount)
	}
	if err := s.MarkUnread(ctx, testRoom, true); err != nil {
		t.Fatal(err)
	}
	room, _ = s.GetRoom(ctx, testRoom)
	if room.UnreadCount != 1 {
		t.Errorf("marking unread must keep count >= 1: got %d", room.UnreadCount)
	}
}

func TestChatListingFiltersAndPreview(t *testing.T) {
	ctx := context.Background()
	s := openTest(t)
	other := id.RoomID("!demo.other:chirper.local")
	if err := s.UpsertRoom(ctx, &Room{RoomID: other, BridgeID: "demo", CreatedTS: 1}); err != nil {
		t.Fatal(err)
	}
	if err := s.SetRoomFields(ctx, testRoom, RoomFields{Name: strPtr("Alice")}); err != nil {
		t.Fatal(err)
	}
	if err := s.SetRoomFields(ctx, other, RoomFields{Name: strPtr("Team"), Archived: boolPtr(true)}); err != nil {
		t.Fatal(err)
	}
	long := strings.Repeat("é", 200)
	mustInsert(t, s, textEvent("long", "alice", 100, long))

	chats, err := s.ListChats(ctx, ChatFilter{})
	if err != nil {
		t.Fatal(err)
	}
	if len(chats) != 1 || chats[0].RoomID != testRoom {
		t.Fatalf("non-archived listing wrong: %+v", chats)
	}
	c := chats[0]
	if c.Preview == nil || len([]rune(c.Preview.Body)) != previewRunes || c.Preview.Type != "text" {
		t.Errorf("preview not truncated to %d runes: %+v", previewRunes, c.Preview)
	}
	if !c.IsGroup || c.IsDM {
		t.Errorf("room without DM flag should be a group")
	}

	single, err := s.GetChat(ctx, testRoom)
	if err != nil || single.Name != "Alice" || single.Preview == nil {
		t.Errorf("GetChat wrong: %+v %v", single, err)
	}
	if _, err := s.GetChat(ctx, "!missing:chirper.local"); err != ErrNotFound {
		t.Errorf("GetChat unknown room: want ErrNotFound, got %v", err)
	}

	archived, _ := s.ListChats(ctx, ChatFilter{Archived: true})
	if len(archived) != 1 || archived[0].RoomID != other {
		t.Errorf("archived listing wrong: %+v", archived)
	}
	byQuery, _ := s.ListChats(ctx, ChatFilter{Query: "lic"})
	if len(byQuery) != 1 {
		t.Errorf("name query should match case-insensitively substring: %+v", byQuery)
	}
	unread, _ := s.ListChats(ctx, ChatFilter{UnreadOnly: true})
	if len(unread) != 0 {
		t.Errorf("no unread chats expected: %+v", unread)
	}
	if err := s.DeleteRoom(ctx, testRoom); err != nil {
		t.Fatal(err)
	}
	after, _ := s.ListChats(ctx, ChatFilter{})
	if len(after) != 0 {
		t.Errorf("deleted rooms must be hidden: %+v", after)
	}
}

func TestListChatsDirectAndServersFiltering(t *testing.T) {
	ctx := context.Background()
	s := openTest(t)
	serverSpace := id.RoomID("!space:chirper.local")
	if err := s.UpsertRoom(ctx, &Room{RoomID: serverSpace, BridgeID: "discord", IsSpace: true, CreatedTS: 1}); err != nil {
		t.Fatal(err)
	}
	if err := s.SetRoomFields(ctx, serverSpace, RoomFields{Name: strPtr("My Guild")}); err != nil {
		t.Fatal(err)
	}
	channel := id.RoomID("!channel:chirper.local")
	if err := s.UpsertRoom(ctx, &Room{RoomID: channel, BridgeID: "discord", Name: "general", ParentID: string(serverSpace), CreatedTS: 2}); err != nil {
		t.Fatal(err)
	}
	dm := id.RoomID("!dm:chirper.local")
	if err := s.UpsertRoom(ctx, &Room{RoomID: dm, BridgeID: "discord", Name: "Bob", IsDM: true, CreatedTS: 3}); err != nil {
		t.Fatal(err)
	}

	direct, err := s.ListChats(ctx, ChatFilter{Network: "discord", DirectOnly: true})
	if err != nil {
		t.Fatal(err)
	}
	if len(direct) != 1 || direct[0].RoomID != dm {
		t.Fatalf("DirectOnly want [dm], got: %+v", direct)
	}

	servers, err := s.ListChats(ctx, ChatFilter{Network: "discord", ServersOnly: true})
	if err != nil {
		t.Fatal(err)
	}
	if len(servers) != 1 || servers[0].RoomID != channel {
		t.Fatalf("ServersOnly want [channel], got: %+v", servers)
	}
	if servers[0].ServerName != "My Guild" {
		t.Errorf("ServerName want 'My Guild', got %q", servers[0].ServerName)
	}
	if !servers[0].IsChannel || servers[0].IsGroup {
		t.Errorf("expected channel to have IsChannel=true and IsGroup=false")
	}
}

func TestPreviewLabelsForMedia(t *testing.T) {
	ctx := context.Background()
	s := openTest(t)
	voice, _ := json.Marshal(map[string]any{
		"msgtype": "m.audio", "body": "voice.ogg", "url": "mxc://chirper.local/v1",
		"org.matrix.msc3245.voice": map[string]any{},
		"info":                     map[string]any{"mimetype": "audio/ogg", "duration": 1500},
	})
	mustInsert(t, s, &Event{EventID: id2("voice"), RoomID: testRoom, Sender: mustUser("alice"),
		Type: typeMessage, ContentJSON: string(voice), TS: 100})
	chat, err := s.GetChat(ctx, testRoom)
	if err != nil {
		t.Fatal(err)
	}
	preview := chat.Preview
	if preview == nil || preview.Type != "voice" || preview.Body != "🎤 Voice message" {
		t.Errorf("voice preview wrong: %+v", preview)
	}
	msg, _ := s.MessageByID(ctx, id2("voice"))
	if msg.Media == nil || msg.Media.URL != "mxc://chirper.local/v1" || msg.Media.DurationMs != 1500 || msg.Media.Mime != "audio/ogg" {
		t.Errorf("voice media wrong: %+v", msg.Media)
	}

	file, _ := json.Marshal(map[string]any{"msgtype": "m.file", "body": "report.pdf", "filename": "report.pdf", "url": "mxc://chirper.local/f"})
	mustInsert(t, s, &Event{EventID: id2("file"), RoomID: testRoom, Sender: mustUser("alice"),
		Type: typeMessage, ContentJSON: string(file), TS: 200})
	msg, _ = s.MessageByID(ctx, id2("file"))
	if msg.Type != "file" || msg.Media.FileName != "report.pdf" {
		t.Errorf("file message wrong: %+v", msg)
	}

	sticker, _ := json.Marshal(map[string]any{"body": "wave", "url": "mxc://chirper.local/s"})
	mustInsert(t, s, &Event{EventID: id2("sticker"), RoomID: testRoom, Sender: mustUser("alice"),
		Type: typeSticker, ContentJSON: string(sticker), TS: 300})
	chat, _ = s.GetChat(ctx, testRoom)
	preview = chat.Preview
	if preview.Body != "Sticker" || preview.Type != "sticker" {
		t.Errorf("sticker preview wrong: %+v", preview)
	}
}

func TestReplyPreviewAndStateEvents(t *testing.T) {
	ctx := context.Background()
	s := openTest(t)
	mustInsert(t, s, textEvent("parent", "alice", 100, "original"))

	reply, _ := json.Marshal(event.MessageEventContent{
		MsgType: event.MsgText, Body: "re",
		RelatesTo: &event.RelatesTo{InReplyTo: &event.InReplyTo{EventID: id2("parent")}},
	})
	mustInsert(t, s, &Event{EventID: id2("reply"), RoomID: testRoom, Sender: SelfUserID,
		Type: typeMessage, ContentJSON: string(reply), TS: 110})
	msg, _ := s.MessageByID(ctx, id2("reply"))
	if msg.ReplyTo == nil || msg.ReplyTo.Body != "original" || msg.ReplyTo.SenderName == "" {
		t.Errorf("reply preview wrong: %+v", msg.ReplyTo)
	}
	if !msg.FromMe || msg.Status != "sent" {
		t.Errorf("own message defaults wrong: fromMe=%v status=%q", msg.FromMe, msg.Status)
	}

	if err := s.SetProfile(ctx, &Profile{UserID: mustUser("alice"), Displayname: "Alice A", ExtraJSON: "{}"}); err != nil {
		t.Fatal(err)
	}
	if err := s.UpsertMember(ctx, &Member{RoomID: testRoom, UserID: mustUser("alice"), Membership: event.MembershipJoin}); err != nil {
		t.Fatal(err)
	}
	name := `{"name":"Team"}`
	mustInsert(t, s, &Event{EventID: id2("state"), RoomID: testRoom, Sender: mustUser("alice"),
		Type: "m.room.name", StateKey: strPtr(""), ContentJSON: name, TS: 120})
	state, _ := s.MessageByID(ctx, id2("state"))
	if state.Type != "state" || state.Body != "Alice A changed the group name to Team" {
		t.Errorf("state body wrong: type=%q body=%q", state.Type, state.Body)
	}
	room, _ := s.GetRoom(ctx, testRoom)
	if room.LastEventID != id2("reply") {
		t.Errorf("state events must not become the preview event: last=%s", room.LastEventID)
	}
	if room.LastActivityTS != 120 {
		t.Errorf("state events still count as activity: got %d", room.LastActivityTS)
	}
}

func TestSearchAndProfilePropagation(t *testing.T) {
	ctx := context.Background()
	s := openTest(t)
	if err := s.UpsertMember(ctx, &Member{RoomID: testRoom, UserID: mustUser("bob"), Membership: event.MembershipJoin}); err != nil {
		t.Fatal(err)
	}
	mustInsert(t, s, textEvent("s1", "bob", 100, "meet at 100% café"))
	mustInsert(t, s, textEvent("s2", "bob", 101, "unrelated"))

	hits, err := s.Search(ctx, "100%", 10)
	if err != nil {
		t.Fatal(err)
	}
	if len(hits) != 1 || hits[0].EventID != id2("s1") {
		t.Errorf("search wrong: %+v", hits)
	}

	if err := s.SetProfile(ctx, &Profile{UserID: mustUser("bob"), Displayname: "Bobby", AvatarURL: "mxc://chirper.local/a", ExtraJSON: "{}"}); err != nil {
		t.Fatal(err)
	}
	member, _ := s.GetMember(ctx, testRoom, mustUser("bob"))
	if member.Displayname != "Bobby" || member.AvatarURL != "mxc://chirper.local/a" {
		t.Errorf("profile not propagated to member row: %+v", member)
	}
}

func TestMediaRoundTrip(t *testing.T) {
	ctx := context.Background()
	s := openTest(t)
	if _, err := s.GetMedia(ctx, "nope"); err != ErrNotFound {
		t.Fatalf("missing media: want ErrNotFound, got %v", err)
	}
	m := &Media{MediaID: "abc", Mime: "image/png", Size: 4, FileName: "a.png", Path: "/tmp/abc", CreatedTS: 1}
	if err := s.PutMedia(ctx, m); err != nil {
		t.Fatal(err)
	}
	got, err := s.GetMedia(ctx, "abc")
	if err != nil {
		t.Fatal(err)
	}
	if got.Mime != "image/png" || got.Path != "/tmp/abc" || got.EncryptedKeyJSON != nil {
		t.Errorf("media round trip wrong: %+v", got)
	}
}

func TestMessageJSONShape(t *testing.T) {
	ctx := context.Background()
	s := openTest(t)
	mustInsert(t, s, textEvent("j", "alice", 100, "hi"))
	msg, _ := s.MessageByID(ctx, id2("j"))
	raw, err := json.Marshal(msg)
	if err != nil {
		t.Fatal(err)
	}
	var m map[string]any
	_ = json.Unmarshal(raw, &m)
	for _, key := range []string{"eventId", "roomId", "order", "senderName", "fromMe", "ts", "type", "body",
		"formattedBody", "media", "replyTo", "edited", "redacted", "reactions", "status", "error", "live"} {
		if _, ok := m[key]; !ok {
			t.Errorf("message JSON missing %q: %s", key, raw)
		}
	}
	if m["media"] != nil || m["replyTo"] != nil {
		t.Errorf("media and replyTo must be null for text: %s", raw)
	}
}

// TestOlderEventKeepsThePreview checks that an event older than the newest one (backfill) does not
// become the chat preview, while a newer event does.
func TestOlderEventKeepsThePreview(t *testing.T) {
	ctx := context.Background()
	s := openTest(t)
	mustInsert(t, s, textEvent("new", "alice", 2000, "newest"))
	mustInsert(t, s, textEvent("old", "alice", 1000, "backfilled"))
	room, err := s.GetRoom(ctx, testRoom)
	if err != nil {
		t.Fatal(err)
	}
	if room.LastEventID != id2("new") || room.LastActivityTS != 2000 {
		t.Fatalf("older insert moved the preview: last=%s activity=%d", room.LastEventID, room.LastActivityTS)
	}
	mustInsert(t, s, textEvent("newer", "alice", 3000, "newer"))
	if room, _ = s.GetRoom(ctx, testRoom); room.LastEventID != id2("newer") {
		t.Errorf("newer insert did not become the preview: %s", room.LastEventID)
	}
}

func strPtr(s string) *string { return &s }
func boolPtr(b bool) *bool    { return &b }
