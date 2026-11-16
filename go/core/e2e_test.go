package core_test

import (
	"context"
	"crypto/rand"
	"encoding/json"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/rs/zerolog"
	"go.mau.fi/util/dbutil"
	_ "go.mau.fi/util/dbutil/litestream" // registers the sqlite3-fk-wal driver the bridges open
	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/bridgeconfig"
	"maunium.net/go/mautrix/bridgev2/commands"
	"maunium.net/go/mautrix/bridgev2/networkid"
	"maunium.net/go/mautrix/bridgev2/status"
	"maunium.net/go/mautrix/event"
	"maunium.net/go/mautrix/id"

	"chirper.dev/core/engine"
	"chirper.dev/core/facade"
	"chirper.dev/core/internal/testnet"
	"chirper.dev/core/localmx"
	"chirper.dev/core/security"
	"chirper.dev/core/store"
)

// e2eSink records the sink notifications of the connector.
type e2eSink struct {
	mu       sync.Mutex
	statuses map[id.EventID]event.MessageStatus
}

func (s *e2eSink) RoomChanged(id.RoomID)                              {}
func (s *e2eSink) EventChanged(id.RoomID, id.EventID, bool)           {}
func (s *e2eSink) EventRemoved(id.RoomID, id.EventID)                 {}
func (s *e2eSink) Typing(id.RoomID, []id.UserID)                      {}
func (s *e2eSink) BridgeState(networkid.BridgeID, status.BridgeState) {}
func (s *e2eSink) MessageStatus(_ id.RoomID, eventID id.EventID, st *bridgev2.MessageStatus) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.statuses[eventID] = st.Status
}

func (s *e2eSink) status(eventID id.EventID) (event.MessageStatus, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	st, ok := s.statuses[eventID]
	return st, ok
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

func chatByName(t *testing.T, st *store.Store, name string) (store.Chat, bool) {
	t.Helper()
	chats, err := st.ListChats(context.Background(), store.ChatFilter{})
	if err != nil {
		t.Fatal(err)
	}
	for _, c := range chats {
		if c.Name == name {
			return c, true
		}
	}
	return store.Chat{}, false
}

func messageWithBody(t *testing.T, st *store.Store, roomID id.RoomID, body string) (store.Message, bool) {
	t.Helper()
	msgs, err := st.ListMessages(context.Background(), roomID, 0, 200)
	if err != nil {
		t.Fatal(err)
	}
	for _, m := range msgs {
		if m.Body == body {
			return m, true
		}
	}
	return store.Message{}, false
}

func TestDemoNetworkEndToEnd(t *testing.T) {
	ctx := context.Background()
	dir := t.TempDir()
	logger := zerolog.New(zerolog.NewTestWriter(t)).Level(zerolog.WarnLevel)

	st, err := store.Open(ctx, filepath.Join(dir, "chirper.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = st.Close() })

	sink := &e2eSink{statuses: map[id.EventID]event.MessageStatus{}}
	connector := localmx.New(testnet.BridgeID, st, filepath.Join(dir, "media"), sink)

	bridgeDB, err := dbutil.NewFromConfig(string(testnet.BridgeID), dbutil.Config{
		PoolConfig: dbutil.PoolConfig{
			Type:         "sqlite3-fk-wal",
			URI:          filepath.Join(dir, "bridge-demo.db") + "?_txlock=immediate",
			MaxOpenConns: 1,
			MaxIdleConns: 1,
		},
	}, dbutil.ZeroLogger(logger))
	if err != nil {
		t.Fatal(err)
	}

	admin := bridgeconfig.PermissionLevelAdmin
	cfg := &bridgeconfig.BridgeConfig{
		CommandPrefix:     "!demo",
		PortalEventBuffer: 64,
		Backfill:          bridgeconfig.BackfillConfig{Enabled: true, MaxInitialMessages: 20, MaxCatchupMessages: 20},
		Permissions:       bridgeconfig.PermissionConfig{string(store.SelfUserID): &admin},
	}
	network := testnet.New()
	br := bridgev2.NewBridge(testnet.BridgeID, bridgeDB, logger, cfg, connector, network, commands.NewProcessor)
	if err := br.Start(ctx); err != nil {
		t.Fatalf("bridge start: %v", err)
	}
	t.Cleanup(br.Stop)

	user, err := br.GetUserByMXID(ctx, store.SelfUserID)
	if err != nil {
		t.Fatal(err)
	}
	proc, err := network.CreateLogin(ctx, user, "demo")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := proc.Start(ctx); err != nil {
		t.Fatal(err)
	}
	step, err := proc.(bridgev2.LoginProcessUserInput).SubmitUserInput(ctx, map[string]string{"name": "tester"})
	if err != nil {
		t.Fatalf("login: %v", err)
	}
	if step.Type != bridgev2.LoginStepTypeComplete {
		t.Fatalf("login step %s, want complete", step.Type)
	}

	// The seeded chats arrive through the bridge and land in the store.
	eventually(t, "two seeded chats with five messages each", func() bool {
		chats, err := st.ListChats(ctx, store.ChatFilter{})
		if err != nil || len(chats) != 2 {
			return false
		}
		for _, c := range chats {
			msgs, err := st.ListMessages(ctx, c.RoomID, 0, 50)
			if err != nil || len(msgs) < 5 {
				return false
			}
		}
		return true
	})
	dm, ok := chatByName(t, st, "Alice")
	if !ok || !dm.IsDM {
		t.Fatalf("DM chat with Alice missing or not a DM: %+v", dm)
	}
	team, ok := chatByName(t, st, "Team")
	if !ok || !team.IsGroup || team.IsDM {
		t.Fatalf("group chat Team missing or wrong kind: %+v", team)
	}
	if _, ok := messageWithBody(t, st, dm.RoomID, "Hi! This is a demo chat."); !ok {
		t.Error("seeded DM message missing")
	}
	teamMsgs, _ := st.ListMessages(ctx, team.RoomID, 0, 50)
	if len(teamMsgs) != 5 || teamMsgs[0].SenderName != "Bob" {
		t.Errorf("team history wrong: %d messages, first sender %q", len(teamMsgs), teamMsgs[0].SenderName)
	}
	if !teamMsgs[2].FromMe {
		t.Errorf("own message in team chat not marked fromMe: %+v", teamMsgs[2])
	}

	// Send a message the way the UI does: store it as sending, then hand it to the bridge.
	now := time.Now().UnixMilli()
	localID := id.EventID("$local.e2e-hello")
	if _, err := st.InsertEvent(ctx, &store.Event{
		EventID: localID, RoomID: dm.RoomID, Sender: store.SelfUserID, Type: "m.room.message",
		ContentJSON: `{"msgtype":"m.text","body":"hello from the UI"}`, TS: now, Status: "sending",
	}); err != nil {
		t.Fatal(err)
	}
	res := br.QueueMatrixEvent(ctx, &event.Event{
		Type: event.EventMessage, RoomID: dm.RoomID, Sender: store.SelfUserID, ID: localID, Timestamp: now,
		Content: event.Content{Parsed: &event.MessageEventContent{MsgType: event.MsgText, Body: "hello from the UI"}},
	})
	if !res.Success {
		t.Fatalf("bridge rejected the outgoing message: %+v", res)
	}

	eventually(t, "outgoing message marked sent", func() bool {
		m, err := st.MessageByID(ctx, localID)
		return err == nil && m.Status == "sent"
	})

	// The fake contact answers and reacts a moment later.
	eventually(t, "echo reply from Alice", func() bool {
		_, ok := messageWithBody(t, st, dm.RoomID, "echo: hello from the UI")
		return ok
	})
	eventually(t, "Alice's reaction on the outgoing message", func() bool {
		m, err := st.MessageByID(ctx, localID)
		if err != nil || len(m.Reactions) == 0 {
			return false
		}
		// Bridged emoji keys carry the U+FE0F variation selector that bridgev2 adds.
		key := strings.ReplaceAll(m.Reactions[0].Key, "\uFE0F", "")
		return key == "👍" && m.Reactions[0].Count == 1 && strings.Contains(strings.Join(m.Reactions[0].Senders, ","), "Alice")
	})

	// Edit the outgoing message through the bridge. The network echoes the edit back, and the
	// store applies it.
	edit := &event.MessageEventContent{MsgType: event.MsgText, Body: "hello, edited"}
	edit.SetEdit(localID)
	editID := id.EventID("$local.e2e-edit")
	if res := br.QueueMatrixEvent(ctx, &event.Event{Type: event.EventMessage, RoomID: dm.RoomID,
		Sender: store.SelfUserID, ID: editID, Timestamp: now, Content: event.Content{Parsed: edit}}); !res.Success {
		t.Fatalf("bridge rejected the edit: %+v", res)
	}
	eventually(t, "edit applied to the stored message", func() bool {
		m, err := st.MessageByID(ctx, localID)
		return err == nil && m.Body == "hello, edited" && m.Edited
	})

	// React to it. The network accepts the reaction; the store's reaction rows are the engine's to write.
	reactID := id.EventID("$local.e2e-react")
	if res := br.QueueMatrixEvent(ctx, &event.Event{Type: event.EventReaction, RoomID: dm.RoomID,
		Sender: store.SelfUserID, ID: reactID, Timestamp: now, Content: event.Content{Parsed: &event.ReactionEventContent{
			RelatesTo: event.RelatesTo{Type: event.RelAnnotation, EventID: localID, Key: "❤"},
		}}}); !res.Success {
		t.Fatalf("bridge rejected the reaction: %+v", res)
	}

	// Redact the outgoing message. The network accepts the removal; the store's copy is cleared by
	// the engine, not by the bridge, because bridgev2 forgets a message once its redaction is handled.
	redactID := id.EventID("$local.e2e-redact")
	if res := br.QueueMatrixEvent(ctx, &event.Event{Type: event.EventRedaction, RoomID: dm.RoomID,
		Sender: store.SelfUserID, ID: redactID, Timestamp: now, Content: event.Content{Parsed: &event.RedactionEventContent{
			Redacts: localID,
		}}}); !res.Success {
		t.Fatalf("bridge rejected the redaction: %+v", res)
	}

	for _, eid := range []id.EventID{editID, reactID, redactID} {
		eventually(t, "bridge success status for "+string(eid), func() bool {
			st, ok := sink.status(eid)
			return ok && st == event.MessageStatusSuccess
		})
	}
}

// nopListener ignores the core's events.
type nopListener struct{}

func (nopListener) OnEvent(string, string) {}

// TestDatabasesAreEncryptedAtRest logs a demo account in with a database key, stops the core, and
// checks every database file with security.VerifyEncrypted. It skips when the build has no SQLCipher.
func TestDatabasesAreEncryptedAtRest(t *testing.T) {
	if !security.EncryptionAvailable {
		t.Skip("this build has no SQLCipher: build with -tags sqlcipher,libsqlite3")
	}
	dir := t.TempDir()
	key := make([]byte, engine.DBKeyBytes)
	if _, err := rand.Read(key); err != nil {
		t.Fatal(err)
	}
	c, err := facade.NewCoreWithKey(dir, key, nopListener{})
	if err != nil {
		t.Fatal(err)
	}
	if err := c.SetSettings(`{"demoNetwork":true}`); err != nil {
		t.Fatal(err)
	}
	if err := c.Start(); err != nil {
		t.Fatal(err)
	}
	raw, err := c.StartLogin("demo", "demo")
	if err != nil {
		t.Fatal(err)
	}
	var step struct {
		ProcessID string `json:"processId"`
	}
	if err := json.Unmarshal([]byte(raw), &step); err != nil {
		t.Fatal(err)
	}
	if _, err := c.SubmitLoginInput(step.ProcessID, `{"name":"tester"}`); err != nil {
		t.Fatalf("login: %v", err)
	}
	deadline := time.Now().Add(20 * time.Second)
	for !strings.Contains(c.ListChats(""), `"roomId"`) {
		if time.Now().After(deadline) {
			t.Fatal("seeded chats never arrived")
		}
		time.Sleep(20 * time.Millisecond)
	}
	c.Stop()

	files, err := filepath.Glob(filepath.Join(dir, "*.db"))
	if err != nil {
		t.Fatal(err)
	}
	if !slices.Contains(files, filepath.Join(dir, "chirper.db")) {
		t.Fatalf("chirper.db not written: %v", files)
	}
	for _, path := range files {
		if err := security.VerifyEncrypted(path, key); err != nil {
			t.Errorf("%s: %v", filepath.Base(path), err)
		}
	}
}
