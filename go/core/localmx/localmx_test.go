package localmx

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"maunium.net/go/mautrix"
	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/database"
	"maunium.net/go/mautrix/bridgev2/networkid"
	"maunium.net/go/mautrix/bridgev2/status"
	"maunium.net/go/mautrix/event"
	"maunium.net/go/mautrix/id"

	"chirper.dev/core/store"
)

// recordingSink remembers the notifications the connector emits.
type recordingSink struct {
	mu       sync.Mutex
	rooms    int
	changed  []id.EventID
	live     map[id.EventID]bool
	removed  []id.EventID
	typing   [][]id.UserID
	statuses []id.EventID
}

func (r *recordingSink) RoomChanged(id.RoomID) { r.mu.Lock(); r.rooms++; r.mu.Unlock() }
func (r *recordingSink) EventChanged(_ id.RoomID, e id.EventID, live bool) {
	r.mu.Lock()
	if r.live == nil {
		r.live = map[id.EventID]bool{}
	}
	r.live[e] = live
	r.changed = append(r.changed, e)
	r.mu.Unlock()
}
func (r *recordingSink) EventRemoved(_ id.RoomID, e id.EventID) {
	r.mu.Lock()
	r.removed = append(r.removed, e)
	r.mu.Unlock()
}
func (r *recordingSink) Typing(_ id.RoomID, users []id.UserID) {
	r.mu.Lock()
	r.typing = append(r.typing, users)
	r.mu.Unlock()
}
func (r *recordingSink) BridgeState(networkid.BridgeID, status.BridgeState) {}
func (r *recordingSink) MessageStatus(_ id.RoomID, e id.EventID, _ *bridgev2.MessageStatus) {
	r.mu.Lock()
	r.statuses = append(r.statuses, e)
	r.mu.Unlock()
}

const testBridge networkid.BridgeID = "demo"

var testRoom = id.RoomID("!demo.room1:chirper.local")

type fixture struct {
	t    *testing.T
	st   *store.Store
	c    *Connector
	sink *recordingSink
	dir  string
}

func newFixture(t *testing.T) *fixture {
	t.Helper()
	dir := t.TempDir()
	st, err := store.Open(context.Background(), filepath.Join(dir, "chirper.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = st.Close() })
	sink := &recordingSink{}
	c := New(testBridge, st, filepath.Join(dir, "media"), sink)
	if err := c.Start(context.Background()); err != nil {
		t.Fatal(err)
	}
	if err := st.UpsertRoom(context.Background(), &store.Room{RoomID: testRoom, BridgeID: "demo", CreatedTS: 1}); err != nil {
		t.Fatal(err)
	}
	return &fixture{t: t, st: st, c: c, sink: sink, dir: dir}
}

func (f *fixture) ctx() context.Context { return context.Background() }

func (f *fixture) msgContent(c event.MessageEventContent) *event.Content {
	return &event.Content{Parsed: &c}
}

func TestGhostIDsRoundTripAndAreBridgeScoped(t *testing.T) {
	f := newFixture(t)
	mxid := f.c.FormatGhostMXID("user/with spaces:x")
	if !strings.HasPrefix(string(mxid), "@demo_") || !strings.HasSuffix(string(mxid), ":chirper.local") {
		t.Fatalf("unexpected ghost MXID %s", mxid)
	}
	back, ok := f.c.ParseGhostMXID(mxid)
	if !ok || back != "user/with spaces:x" {
		t.Fatalf("round trip failed: %q %v", back, ok)
	}
	other := New("slack", f.st, f.dir, f.sink)
	if _, ok := other.ParseGhostMXID(mxid); ok {
		t.Error("another bridge must not parse this ghost")
	}
	for _, bad := range []id.UserID{"@demo_x:example.com", "@me:chirper.local", "@demobot:chirper.local", "@demo_:chirper.local"} {
		if _, ok := f.c.ParseGhostMXID(bad); ok {
			t.Errorf("ParseGhostMXID accepted %s", bad)
		}
	}
	if f.c.botUserID() != "@demobot:chirper.local" {
		t.Errorf("bot MXID wrong: %s", f.c.botUserID())
	}
}

func TestDeterministicIDsAreStableAndPrefixed(t *testing.T) {
	f := newFixture(t)
	key := networkid.PortalKey{ID: "chat-1"}
	room1 := f.c.GenerateDeterministicRoomID(key)
	room2 := f.c.GenerateDeterministicRoomID(key)
	if room1 != room2 || !strings.HasPrefix(string(room1), "!demo.") || !strings.HasSuffix(string(room1), ":chirper.local") {
		t.Errorf("room ID not deterministic/prefixed: %s %s", room1, room2)
	}
	ev1 := f.c.GenerateDeterministicEventID(room1, key, "m1", "")
	ev2 := f.c.GenerateDeterministicEventID(room1, key, "m1", "")
	if ev1 != ev2 || !strings.HasPrefix(string(ev1), "$demo.") {
		t.Errorf("event ID not deterministic/prefixed: %s", ev1)
	}
	if ev1 == f.c.GenerateDeterministicEventID(room1, key, "m1", "part2") {
		t.Error("part ID must change the event ID")
	}
	if !strings.HasPrefix(string(newLocalEventID()), "$local.") {
		t.Error("local event IDs must use the $local. prefix")
	}
}

func TestParseContentAcceptsParsedAndRawOnly(t *testing.T) {
	parsed := &event.Content{Parsed: &event.MessageEventContent{MsgType: event.MsgText, Body: "typed"}}
	got, err := parseContent[event.MessageEventContent](parsed)
	if err != nil || got.Body != "typed" {
		t.Fatalf("parsed content: %+v %v", got, err)
	}
	raw := &event.Content{Raw: map[string]any{"msgtype": "m.text", "body": "raw"}}
	got, err = parseContent[event.MessageEventContent](raw)
	if err != nil || got.Body != "raw" {
		t.Fatalf("raw content: %+v %v", got, err)
	}
	// Marshalling a content with both Raw and Parsed keeps the custom field too.
	mixed := &event.Content{
		Parsed: &event.MessageEventContent{MsgType: event.MsgText, Body: "mixed"},
		Raw:    map[string]any{"com.example.field": "kept"},
	}
	b, err := json.Marshal(mixed)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Contains(b, []byte(`"com.example.field":"kept"`)) || !bytes.Contains(b, []byte(`"body":"mixed"`)) {
		t.Errorf("mixed content lost fields: %s", b)
	}
}

func TestSendMessageStoresTextAndCountsUnread(t *testing.T) {
	ctx := context.Background()
	f := newFixture(t)
	ts := time.Now()
	alice, _ := f.c.GhostIntent("alice").(*intent)
	if alice == nil {
		t.Fatal("ghost intent has unexpected type")
	}
	if err := alice.SetDisplayName(ctx, "Alice"); err != nil {
		t.Fatal(err)
	}
	resp, err := alice.SendMessage(ctx, testRoom, event.EventMessage,
		f.msgContent(event.MessageEventContent{MsgType: event.MsgText, Body: "hi there"}),
		&bridgev2.MatrixSendExtra{Timestamp: ts, MessageMeta: &database.Message{ID: "m1"}})
	if err != nil {
		t.Fatal(err)
	}
	if resp.EventID != f.c.GenerateDeterministicEventID(testRoom, networkid.PortalKey{}, "m1", "") {
		t.Errorf("deterministic ID not used: %s", resp.EventID)
	}
	msgs, err := f.st.ListMessages(ctx, testRoom, 0, 10)
	if err != nil || len(msgs) != 1 {
		t.Fatalf("ListMessages: %v %+v", err, msgs)
	}
	if msgs[0].Body != "hi there" || msgs[0].SenderName != "Alice" || msgs[0].TS != ts.UnixMilli() {
		t.Errorf("stored message wrong: %+v", msgs[0])
	}
	room, _ := f.st.GetRoom(ctx, testRoom)
	if room.UnreadCount != 1 {
		t.Errorf("incoming message should be unread: %d", room.UnreadCount)
	}
	if len(f.sink.changed) == 0 {
		t.Error("sink not notified about the new event")
	}
	if !f.sink.live[resp.EventID] {
		t.Error("a message received now must be reported as live")
	}
}

func TestEditRedactAndReactionRelations(t *testing.T) {
	ctx := context.Background()
	f := newFixture(t)
	bob := f.c.GhostIntent("bob")
	resp, err := bob.SendMessage(ctx, testRoom, event.EventMessage,
		f.msgContent(event.MessageEventContent{MsgType: event.MsgText, Body: "original"}), nil)
	if err != nil {
		t.Fatal(err)
	}
	target := resp.EventID

	// Edits in the shape bridgev2 builds them: "* body" outside, the real content in m.new_content.
	edit := &event.MessageEventContent{MsgType: event.MsgText, Body: "* edited"}
	edit.SetEdit(target)
	edit.NewContent.Body = "edited"
	if _, err := bob.SendMessage(ctx, testRoom, event.EventMessage, &event.Content{Parsed: edit}, nil); err != nil {
		t.Fatal(err)
	}
	msg, err := f.st.MessageByID(ctx, target)
	if err != nil {
		t.Fatal(err)
	}
	if msg.Body != "edited" || !msg.Edited {
		t.Errorf("edit not applied from Parsed NewContent: %+v", msg)
	}

	// Reactions by another user, then redaction of that reaction event.
	carol := f.c.GhostIntent("carol")
	reaction := &event.Content{Parsed: &event.ReactionEventContent{RelatesTo: event.RelatesTo{
		Type: event.RelAnnotation, EventID: target, Key: "👍",
	}}}
	rresp, err := carol.SendMessage(ctx, testRoom, event.EventReaction, reaction, nil)
	if err != nil {
		t.Fatal(err)
	}
	msg, _ = f.st.MessageByID(ctx, target)
	if len(msg.Reactions) != 1 || msg.Reactions[0].Count != 1 {
		t.Fatalf("reaction not stored: %+v", msg.Reactions)
	}
	red := &event.Content{Parsed: &event.RedactionEventContent{Redacts: rresp.EventID}}
	if _, err := carol.SendMessage(ctx, testRoom, event.EventRedaction, red, nil); err != nil {
		t.Fatal(err)
	}
	msg, _ = f.st.MessageByID(ctx, target)
	if len(msg.Reactions) != 0 {
		t.Errorf("reaction not removed by redaction: %+v", msg.Reactions)
	}
	if len(f.sink.removed) != 1 || f.sink.removed[0] != rresp.EventID {
		t.Errorf("reaction removal not reported to sink: %v", f.sink.removed)
	}

	// Redacting the message itself.
	if _, err := bob.SendMessage(ctx, testRoom, event.EventRedaction,
		&event.Content{Parsed: &event.RedactionEventContent{Redacts: target}}, nil); err != nil {
		t.Fatal(err)
	}
	msg, _ = f.st.MessageByID(ctx, target)
	if !msg.Redacted || msg.Body != "" {
		t.Errorf("message not redacted: %+v", msg)
	}
	// Redacting something unknown is not an error.
	if _, err := bob.SendMessage(ctx, testRoom, event.EventRedaction,
		&event.Content{Parsed: &event.RedactionEventContent{Redacts: "$nope"}}, nil); err != nil {
		t.Errorf("redacting unknown event should succeed: %v", err)
	}
}

func TestEditFromRawContentKeepsCustomNewContent(t *testing.T) {
	ctx := context.Background()
	f := newFixture(t)
	bob := f.c.GhostIntent("bob")
	resp, _ := bob.SendMessage(ctx, testRoom, event.EventMessage,
		f.msgContent(event.MessageEventContent{MsgType: event.MsgText, Body: "one"}), nil)
	raw := &event.Content{
		Parsed: &event.MessageEventContent{MsgType: event.MsgText, Body: "* two", RelatesTo: (&event.RelatesTo{}).SetReplace(resp.EventID)},
		Raw:    map[string]any{"m.new_content": map[string]any{"msgtype": "m.text", "body": "two"}},
	}
	if _, err := bob.SendMessage(ctx, testRoom, event.EventMessage, raw, nil); err != nil {
		t.Fatal(err)
	}
	msg, _ := f.st.MessageByID(ctx, resp.EventID)
	if msg.Body != "two" {
		t.Errorf("raw m.new_content not used: %q", msg.Body)
	}
}

func TestSendStateNameMembersAndDMRules(t *testing.T) {
	ctx := context.Background()
	f := newFixture(t)
	bob := f.c.GhostIntent("bob")
	name := &event.Content{Parsed: &event.RoomNameEventContent{Name: "Team"}}
	if _, err := bob.SendState(ctx, testRoom, event.StateRoomName, "", name, time.Time{}); err != nil {
		t.Fatal(err)
	}
	room, _ := f.st.GetRoom(ctx, testRoom)
	if room.Name != "Team" {
		t.Errorf("room name not stored: %q", room.Name)
	}
	bobID := f.c.FormatGhostMXID("bob")
	join := &event.Content{Parsed: &event.MemberEventContent{Membership: event.MembershipJoin, Displayname: "Bob B"}}
	if _, err := bob.SendState(ctx, testRoom, event.StateMember, string(bobID), join, time.Time{}); err != nil {
		t.Fatal(err)
	}
	members, _ := f.c.GetMembers(ctx, testRoom)
	if members[bobID] == nil || members[bobID].Displayname != "Bob B" {
		t.Errorf("member not stored: %+v", members)
	}
	info, _ := f.c.GetMemberInfo(ctx, testRoom, bobID)
	if info == nil || info.Membership != event.MembershipJoin {
		t.Errorf("GetMemberInfo wrong: %+v", info)
	}
	if missing, err := f.c.GetMemberInfo(ctx, testRoom, "@nobody:chirper.local"); missing != nil || err != nil {
		t.Errorf("unknown member should be nil,nil: %+v %v", missing, err)
	}
	msgs, _ := f.st.ListMessages(ctx, testRoom, 0, 10)
	if len(msgs) != 2 || msgs[0].Type != "state" || msgs[1].Body != "Bob B joined" {
		t.Errorf("state summaries wrong: %+v", msgs)
	}

	// Power levels are accepted but not stored.
	if _, err := bob.SendState(ctx, testRoom, event.StatePowerLevels, "", &event.Content{Parsed: &event.PowerLevelsEventContent{}}, time.Time{}); err != nil {
		t.Errorf("power levels must be accepted: %v", err)
	}
}

func TestCreateRoomStoresStateAndInvites(t *testing.T) {
	ctx := context.Background()
	f := newFixture(t)
	bot := f.c.BotIntent()
	req := &mautrix.ReqCreateRoom{
		Name:                 "Group",
		IsDirect:             false,
		BeeperLocalRoomID:    f.c.GenerateDeterministicRoomID(networkid.PortalKey{ID: "team"}),
		CreationContent:      map[string]any{},
		Invite:               []id.UserID{"@me:chirper.local"},
		BeeperInitialMembers: []id.UserID{f.c.FormatGhostMXID("alice")},
		InitialState: []*event.Event{{
			Type:    event.StateTopic,
			Content: event.Content{Parsed: &event.TopicEventContent{Topic: "topic here"}},
		}},
	}
	roomID, err := bot.CreateRoom(ctx, req)
	if err != nil {
		t.Fatal(err)
	}
	if roomID != req.BeeperLocalRoomID {
		t.Errorf("local room ID not used: %s", roomID)
	}
	room, err := f.st.GetRoom(ctx, roomID)
	if err != nil {
		t.Fatal(err)
	}
	if room.Name != "Group" || room.Topic != "topic here" || room.IsDM {
		t.Errorf("room fields wrong: %+v", room)
	}
	members, _ := f.c.GetMembers(ctx, roomID)
	if members["@me:chirper.local"].Membership != event.MembershipInvite {
		t.Errorf("invite not stored: %+v", members)
	}
	if members[f.c.FormatGhostMXID("alice")].Membership != event.MembershipJoin {
		t.Errorf("initial member not joined: %+v", members)
	}
	if err := bot.DeleteRoom(ctx, roomID, true); err != nil {
		t.Fatal(err)
	}
	chats, _ := f.st.ListChats(ctx, store.ChatFilter{})
	for _, c := range chats {
		if c.RoomID == roomID {
			t.Error("deleted room listed")
		}
	}
}

// TestOldAndBackfilledMessagesAreNotLiveNorUnread checks that only a message received in real time
// counts as unread: history and messages older than the live window never do.
func TestOldAndBackfilledMessagesAreNotLiveNorUnread(t *testing.T) {
	ctx := context.Background()
	f := newFixture(t)
	alice := f.c.GhostIntent("alice").(*intent)
	old := time.Now().Add(-time.Hour)
	if _, err := alice.SendMessage(ctx, testRoom, event.EventMessage,
		f.msgContent(event.MessageEventContent{MsgType: event.MsgText, Body: "an hour old"}),
		&bridgev2.MatrixSendExtra{Timestamp: old, MessageMeta: &database.Message{ID: "old1"}}); err != nil {
		t.Fatal(err)
	}
	backfillID := f.c.GenerateDeterministicEventID(testRoom, networkid.PortalKey{}, "hist1", "")
	if _, err := f.c.BatchSend(ctx, testRoom, &mautrix.ReqBeeperBatchSend{Events: []*event.Event{{
		ID: backfillID, Sender: alice.GetMXID(), Type: event.EventMessage, Timestamp: old.UnixMilli(), RoomID: testRoom,
		Content: event.Content{Parsed: &event.MessageEventContent{MsgType: event.MsgText, Body: "history"}},
	}}}, nil); err != nil {
		t.Fatal(err)
	}
	room, err := f.st.GetRoom(ctx, testRoom)
	if err != nil {
		t.Fatal(err)
	}
	if room.UnreadCount != 0 {
		t.Errorf("old and backfilled messages counted as unread: %d", room.UnreadCount)
	}
	oldID := f.c.GenerateDeterministicEventID(testRoom, networkid.PortalKey{}, "old1", "")
	if f.sink.live[oldID] || f.sink.live[backfillID] {
		t.Errorf("old or backfilled messages reported as live: %v", f.sink.live)
	}
}

func TestBatchSendUsesDeterministicIDsAndNotifiesOnce(t *testing.T) {
	ctx := context.Background()
	f := newFixture(t)
	bob := f.c.GhostIntent("bob")
	evts := []*event.Event{}
	for i := range 3 {
		evts = append(evts, &event.Event{
			ID:        f.c.GenerateDeterministicEventID(testRoom, networkid.PortalKey{}, networkid.MessageID(string(rune('a'+i))), ""),
			Sender:    bob.GetMXID(),
			Type:      event.EventMessage,
			Timestamp: 1000 + int64(i),
			RoomID:    testRoom,
			Content:   event.Content{Parsed: &event.MessageEventContent{MsgType: event.MsgText, Body: string(rune('a' + i))}},
		})
	}
	before := len(f.sink.changed)
	resp, err := f.c.BatchSend(ctx, testRoom, &mautrix.ReqBeeperBatchSend{Events: evts}, nil)
	if err != nil {
		t.Fatal(err)
	}
	if len(resp.EventIDs) != 3 || resp.EventIDs[0] != evts[0].ID {
		t.Fatalf("batch IDs wrong: %+v", resp.EventIDs)
	}
	if got := len(f.sink.changed) - before; got != 1 {
		t.Errorf("batch should notify once for the last event, got %d notifications", got)
	}
	msgs, _ := f.st.ListMessages(ctx, testRoom, 0, 10)
	if len(msgs) != 3 || msgs[0].Body != "a" || msgs[2].Body != "c" {
		t.Errorf("batch order wrong: %+v", msgs)
	}
}

func TestGetEventReturnsStoredContent(t *testing.T) {
	ctx := context.Background()
	f := newFixture(t)
	bob := f.c.GhostIntent("bob")
	resp, _ := bob.SendMessage(ctx, testRoom, event.EventMessage,
		f.msgContent(event.MessageEventContent{MsgType: event.MsgText, Body: "fetch me"}), nil)
	evt, err := bob.GetEvent(ctx, testRoom, resp.EventID)
	if err != nil {
		t.Fatal(err)
	}
	if evt.Type != event.EventMessage || evt.Content.AsMessage().Body != "fetch me" || evt.Sender != bob.GetMXID() {
		t.Errorf("GetEvent wrong: %+v", evt)
	}
	if _, err := bob.GetEvent(ctx, "!other:chirper.local", resp.EventID); !errors.Is(err, store.ErrNotFound) {
		t.Errorf("event from another room must not be found: %v", err)
	}
}

func TestSendMessageStatusUpdatesStoredEvent(t *testing.T) {
	ctx := context.Background()
	f := newFixture(t)
	// A local send is recorded by the engine; here the test records it directly.
	ev := "$local.status-test"
	_, err := f.st.InsertEvent(ctx, &store.Event{EventID: id.EventID(ev), RoomID: testRoom, Sender: store.SelfUserID,
		Type: "m.room.message", ContentJSON: `{"msgtype":"m.text","body":"pending"}`, TS: 5, Status: "sending"})
	if err != nil {
		t.Fatal(err)
	}
	f.c.SendMessageStatus(ctx, &bridgev2.MessageStatus{Status: event.MessageStatusFail, Message: "offline"},
		&bridgev2.MessageStatusEventInfo{RoomID: testRoom, SourceEventID: id.EventID(ev)})
	msg, _ := f.st.MessageByID(ctx, id.EventID(ev))
	if msg.Status != "failed" || msg.Error != "offline" {
		t.Errorf("failure not stored: %+v", msg)
	}
	f.c.SendMessageStatus(ctx, &bridgev2.MessageStatus{Status: event.MessageStatusSuccess},
		&bridgev2.MessageStatusEventInfo{RoomID: testRoom, SourceEventID: id.EventID(ev)})
	msg, _ = f.st.MessageByID(ctx, id.EventID(ev))
	if msg.Status != "sent" || msg.Error != "" {
		t.Errorf("success not stored: %+v", msg)
	}
	if len(f.sink.statuses) != 2 {
		t.Errorf("sink not told about statuses: %v", f.sink.statuses)
	}
}

func TestMediaUploadDownloadRoundTrip(t *testing.T) {
	ctx := context.Background()
	f := newFixture(t)
	bob := f.c.GhostIntent("bob")
	uri, file, err := bob.UploadMedia(ctx, "", []byte("png-bytes"), "a.png", "image/png")
	if err != nil || file != nil {
		t.Fatalf("UploadMedia: %v %v", err, file)
	}
	if !strings.HasPrefix(string(uri), "mxc://chirper.local/") {
		t.Fatalf("unexpected URI %s", uri)
	}
	data, err := bob.DownloadMedia(ctx, uri, nil)
	if err != nil || string(data) != "png-bytes" {
		t.Fatalf("DownloadMedia: %q %v", data, err)
	}
	info, err := os.Stat(filepath.Join(f.dir, "media", strings.TrimPrefix(string(uri), "mxc://chirper.local/")))
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm() != 0o600 {
		t.Errorf("media file mode %v, want 0600", info.Mode().Perm())
	}

	streamURI, _, err := bob.UploadMediaStream(ctx, "", 0, false, func(w io.Writer) (*bridgev2.FileStreamResult, error) {
		_, err := io.WriteString(w, "streamed")
		return &bridgev2.FileStreamResult{FileName: "s.txt", MimeType: "text/plain"}, err
	})
	if err != nil {
		t.Fatal(err)
	}
	var seen string
	err = bob.DownloadMediaToFile(ctx, streamURI, nil, true, func(fh *os.File) error {
		b, err := io.ReadAll(fh)
		seen = string(b)
		_, _ = fh.Write([]byte("!"))
		return err
	})
	if err != nil || seen != "streamed" {
		t.Fatalf("DownloadMediaToFile: %q %v", seen, err)
	}
	again, _ := bob.DownloadMedia(ctx, streamURI, nil)
	if string(again) != "streamed" {
		t.Errorf("writable download modified the stored media: %q", again)
	}
	if _, err := bob.DownloadMedia(ctx, "mxc://elsewhere/x", nil); err == nil {
		t.Error("foreign media must not resolve")
	}
	if _, err := bob.DownloadMedia(ctx, uri, &event.EncryptedFileInfo{}); !errors.Is(err, ErrNotSupported) {
		t.Errorf("encrypted media: want ErrNotSupported, got %v", err)
	}
}

// A joined member who only changes display name or avatar updates the member row but adds no
// timeline entry. Joins and leaves still do.
func TestDisplayNameChangeOfJoinedMemberIsNotShown(t *testing.T) {
	ctx := context.Background()
	f := newFixture(t)
	bob := f.c.GhostIntent("bob")
	bobID := f.c.FormatGhostMXID("bob")
	send := func(membership event.Membership, name string) {
		t.Helper()
		content := &event.Content{Parsed: &event.MemberEventContent{Membership: membership, Displayname: name}}
		if _, err := bob.SendState(ctx, testRoom, event.StateMember, string(bobID), content, time.Time{}); err != nil {
			t.Fatal(err)
		}
	}
	send(event.MembershipJoin, "Bob B")
	send(event.MembershipJoin, "Bobby")
	msgs, _ := f.st.ListMessages(ctx, testRoom, 0, 10)
	// The body is worded from the member's current name, so only the count is checked here.
	if len(msgs) != 1 {
		t.Fatalf("a rename must add no timeline entry; got %+v", msgs)
	}
	info, _ := f.c.GetMemberInfo(ctx, testRoom, bobID)
	if info == nil || info.Displayname != "Bobby" {
		t.Errorf("the member row must still take the new name: %+v", info)
	}
	send(event.MembershipLeave, "Bobby")
	msgs, _ = f.st.ListMessages(ctx, testRoom, 0, 10)
	if len(msgs) != 2 {
		t.Errorf("a leave must still be shown; got %d entries", len(msgs))
	}
}

func TestProfileUpdatesPropagateToMembers(t *testing.T) {
	ctx := context.Background()
	f := newFixture(t)
	bob := f.c.GhostIntent("bob")
	if _, err := bob.SendState(ctx, testRoom, event.StateMember, string(bob.GetMXID()),
		&event.Content{Parsed: &event.MemberEventContent{Membership: event.MembershipJoin}}, time.Time{}); err != nil {
		t.Fatal(err)
	}
	if err := bob.SetDisplayName(ctx, "Bobby"); err != nil {
		t.Fatal(err)
	}
	if err := bob.SetAvatarURL(ctx, "mxc://chirper.local/av"); err != nil {
		t.Fatal(err)
	}
	m, _ := f.st.GetMember(ctx, testRoom, bob.GetMXID())
	if m.Displayname != "Bobby" || m.AvatarURL != "mxc://chirper.local/av" {
		t.Errorf("profile not propagated: %+v", m)
	}
	if err := bob.SetProfile(ctx, map[string]any{"displayname": "Robert", "avatar_url": "", "com.example.pronoun": "they"}); err != nil {
		t.Fatal(err)
	}
	p, _ := f.st.GetProfile(ctx, bob.GetMXID())
	if p.Displayname != "Robert" || !strings.Contains(p.ExtraJSON, "com.example.pronoun") {
		t.Errorf("SetProfile wrong: %+v", p)
	}
	if err := bob.SetExtraProfileMeta(ctx, map[string]any{"network": "demo"}); err != nil {
		t.Fatal(err)
	}
}

func TestTagMuteAndDoublePuppetRules(t *testing.T) {
	ctx := context.Background()
	f := newFixture(t)
	bot := f.c.BotIntent()
	if err := bot.TagRoom(ctx, testRoom, event.RoomTagFavourite, true); err != nil {
		t.Fatal(err)
	}
	if err := bot.TagRoom(ctx, testRoom, event.RoomTagLowPriority, true); err != nil {
		t.Fatal(err)
	}
	room, _ := f.st.GetRoom(ctx, testRoom)
	if !room.Pinned || !room.Archived {
		t.Errorf("tags not applied: pinned=%v archived=%v", room.Pinned, room.Archived)
	}
	if err := bot.MuteRoom(ctx, testRoom, event.MutedForever); err != nil {
		t.Fatal(err)
	}
	room, _ = f.st.GetRoom(ctx, testRoom)
	if room.MutedUntil != -1 {
		t.Errorf("forever mute: got %d", room.MutedUntil)
	}
	if err := bot.MuteRoom(ctx, testRoom, time.Time{}); err != nil {
		t.Fatal(err)
	}
	room, _ = f.st.GetRoom(ctx, testRoom)
	if room.MutedUntil != 0 {
		t.Errorf("unmute: got %d", room.MutedUntil)
	}

	if _, _, err := f.c.NewUserIntent(ctx, "@someone:example.com", ""); !errors.Is(err, ErrNotSupported) {
		t.Errorf("double puppeting another user: %v", err)
	}
	me, token, err := f.c.NewUserIntent(ctx, store.SelfUserID, "")
	if err != nil || token != "local" || !me.IsDoublePuppet() {
		t.Errorf("self double puppet: %v %q", err, token)
	}
	if bob := f.c.GhostIntent("bob"); bob.IsDoublePuppet() {
		t.Error("ghost must not be a double puppet")
	}
}

func TestTypingExpires(t *testing.T) {
	f := newFixture(t)
	bob := f.c.GhostIntent("bob")
	if err := bob.MarkTyping(context.Background(), testRoom, bridgev2.TypingTypeText, 20*time.Millisecond); err != nil {
		t.Fatal(err)
	}
	if len(f.sink.typing) != 1 || len(f.sink.typing[0]) != 1 {
		t.Fatalf("typing start not reported: %v", f.sink.typing)
	}
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		f.sink.mu.Lock()
		n := len(f.sink.typing)
		f.sink.mu.Unlock()
		if n >= 2 {
			break
		}
		time.Sleep(5 * time.Millisecond)
	}
	f.sink.mu.Lock()
	last := f.sink.typing[len(f.sink.typing)-1]
	f.sink.mu.Unlock()
	if len(last) != 0 {
		t.Errorf("typing should expire, still %v", last)
	}
}

func TestUnsupportedFeaturesReturnErrors(t *testing.T) {
	f := newFixture(t)
	if _, err := f.c.GenerateContentURI(context.Background(), networkid.MediaID("x")); !errors.Is(err, ErrNotSupported) {
		t.Errorf("GenerateContentURI: %v", err)
	}
	if _, err := f.c.ParseContentURI(context.Background(), "mxc://other/x"); err == nil {
		t.Error("ParseContentURI must reject foreign URIs")
	}
	bob := f.c.GhostIntent("bob")
	if _, err := bob.SendMessage(context.Background(), testRoom, event.StateRoomName,
		&event.Content{Parsed: &event.RoomNameEventContent{Name: "x"}}, nil); err == nil {
		t.Error("state types must go through SendState")
	}
}
