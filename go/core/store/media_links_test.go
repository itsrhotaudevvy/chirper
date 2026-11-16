package store

import (
	"context"
	"os"
	"path/filepath"
	"reflect"
	"testing"

	"maunium.net/go/mautrix/event"
	"maunium.net/go/mautrix/id"
)

const (
	picContent = `{"msgtype":"m.image","body":"pic.png","url":"mxc://chirper.local/mediaA","info":{"thumbnail_url":"mxc://chirper.local/thumbA"}}`
	otherRoom  = id.RoomID("!other.test:chirper.local")
)

// putMediaFile stores media mediaID with a file on disk and returns the file path.
func putMediaFile(t *testing.T, s *Store, dir, mediaID string) string {
	t.Helper()
	path := filepath.Join(dir, mediaID)
	if err := os.WriteFile(path, []byte("bytes of "+mediaID), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := s.PutMedia(context.Background(), &Media{MediaID: mediaID, Mime: "image/png", Path: path, CreatedTS: 1}); err != nil {
		t.Fatal(err)
	}
	return path
}

func insertPic(t *testing.T, s *Store, eventID id.EventID, roomID id.RoomID, contentJSON string) {
	t.Helper()
	mustInsert(t, s, &Event{EventID: eventID, RoomID: roomID, Sender: "@alice:chirper.local",
		Type: event.EventMessage.Type, ContentJSON: contentJSON, TS: 10})
}

// linksOf returns the media IDs linked to eventID, sorted.
func linksOf(t *testing.T, s *Store, eventID id.EventID) []string {
	t.Helper()
	rows, err := queryAll(context.Background(), s, func(row rowScanner) (string, error) {
		var mediaID string
		return mediaID, row.Scan(&mediaID)
	}, `SELECT media_id FROM event_media WHERE event_id = $1 ORDER BY media_id`, eventID)
	if err != nil {
		t.Fatal(err)
	}
	return rows
}

func fileExists(path string) bool {
	_, err := os.Stat(path)
	return err == nil
}

// TestMigrationLinksMediaOfAVersion2Database opens a database written by the version 2 schema, which
// has no media links, and checks that the version 3 migration links its existing events.
func TestMigrationLinksMediaOfAVersion2Database(t *testing.T) {
	ctx := context.Background()
	dir := t.TempDir()
	path := filepath.Join(dir, "chirper.db")

	old, err := openWithUpgrades(ctx, path, upgradeSteps(2))
	if err != nil {
		t.Fatal(err)
	}
	if err := old.UpsertRoom(ctx, &Room{RoomID: testRoom, BridgeID: "demo", CreatedTS: 1}); err != nil {
		t.Fatal(err)
	}
	mediaPath := putMediaFile(t, old, dir, "mediaA")
	// Rows as the version 2 code wrote them: events and media, with no links.
	mustExecRaw(ctx, t, old, `INSERT INTO event (event_id, room_id, ord, sender, type, content_json, ts)
		VALUES ('$pic', $1, 1, '@alice:chirper.local', 'm.room.message', $2, 10)`, testRoom, picContent)
	mustExecRaw(ctx, t, old, `INSERT INTO event (event_id, room_id, ord, sender, type, content_json, ts)
		VALUES ('$text', $1, 2, '@alice:chirper.local', 'm.room.message', '{"msgtype":"m.text","body":"hi"}', 11)`, testRoom)
	if err := old.Close(); err != nil {
		t.Fatal(err)
	}

	s, err := Open(ctx, path)
	if err != nil {
		t.Fatalf("Open after migration: %v", err)
	}
	t.Cleanup(func() { _ = s.Close() })

	var version int
	if err := s.db.QueryRow(ctx, `SELECT version FROM chirper_version`).Scan(&version); err != nil || version != 3 {
		t.Fatalf("schema version = %d, %v; want 3", version, err)
	}
	if got, want := linksOf(t, s, "$pic"), []string{"mediaA", "thumbA"}; !reflect.DeepEqual(got, want) {
		t.Fatalf("links of the migrated picture = %v, want %v", got, want)
	}
	if got := linksOf(t, s, "$text"); len(got) != 0 {
		t.Fatalf("text message linked to media: %v", got)
	}

	// The backfilled link is what erasure uses: deleting the room removes the file.
	if err := s.DeleteRoom(ctx, testRoom); err != nil {
		t.Fatal(err)
	}
	if fileExists(mediaPath) {
		t.Fatal("media file of a migrated event survived room deletion")
	}
}

func mustExecRaw(ctx context.Context, t *testing.T, s *Store, query string, args ...any) {
	t.Helper()
	if _, err := s.db.Exec(ctx, query, args...); err != nil {
		t.Fatalf("%s: %v", query, err)
	}
}

func TestInsertEventLinksLocalMediaOnly(t *testing.T) {
	s := openTest(t)
	insertPic(t, s, "$pic", testRoom, picContent)
	insertPic(t, s, "$remote", testRoom, `{"url":"mxc://elsewhere.org/abc","info":{}}`)
	if got, want := linksOf(t, s, "$pic"), []string{"mediaA", "thumbA"}; !reflect.DeepEqual(got, want) {
		t.Fatalf("links = %v, want %v", got, want)
	}
	if got := linksOf(t, s, "$remote"); len(got) != 0 {
		t.Fatalf("remote media linked: %v", got)
	}
}

func TestDeleteRoomReleasesItsMediaFiles(t *testing.T) {
	ctx := context.Background()
	s := openTest(t)
	dir := t.TempDir()
	path := putMediaFile(t, s, dir, "mediaA")
	insertPic(t, s, "$pic", testRoom, `{"url":"mxc://chirper.local/mediaA"}`)

	if err := s.DeleteRoom(ctx, testRoom); err != nil {
		t.Fatal(err)
	}
	if fileExists(path) {
		t.Fatal("media file survived room deletion")
	}
	if _, err := s.GetMedia(ctx, "mediaA"); err != ErrNotFound {
		t.Fatalf("media row survived room deletion: %v", err)
	}
}

func TestSharedMediaSurvivesUntilItsLastRoomIsDeleted(t *testing.T) {
	ctx := context.Background()
	s := openTest(t)
	if err := s.UpsertRoom(ctx, &Room{RoomID: otherRoom, BridgeID: "demo", CreatedTS: 1}); err != nil {
		t.Fatal(err)
	}
	path := putMediaFile(t, s, t.TempDir(), "mediaA")
	insertPic(t, s, "$pic", testRoom, `{"url":"mxc://chirper.local/mediaA"}`)
	insertPic(t, s, "$copy", otherRoom, `{"url":"mxc://chirper.local/mediaA"}`)

	if err := s.DeleteRoom(ctx, testRoom); err != nil {
		t.Fatal(err)
	}
	if !fileExists(path) {
		t.Fatal("media still used by another room was deleted")
	}
	if err := s.DeleteRoom(ctx, otherRoom); err != nil {
		t.Fatal(err)
	}
	if fileExists(path) {
		t.Fatal("media left without any room survived")
	}
}

func TestRedactKeepsMediaUntilTheRedactionIsFinal(t *testing.T) {
	ctx := context.Background()
	s := openTest(t)
	path := putMediaFile(t, s, t.TempDir(), "mediaA")
	insertPic(t, s, "$pic", testRoom, `{"url":"mxc://chirper.local/mediaA"}`)

	if _, err := s.Redact(ctx, "$pic"); err != nil {
		t.Fatal(err)
	}
	if !fileExists(path) {
		t.Fatal("redaction deleted the file before it was final")
	}
	if err := s.ReleaseEventMedia(ctx, "$pic"); err != nil {
		t.Fatal(err)
	}
	if fileExists(path) {
		t.Fatal("final redaction left the media file")
	}
	if got := linksOf(t, s, "$pic"); len(got) != 0 {
		t.Fatalf("links left after release: %v", got)
	}
}

func TestPurgeLoginErasesOnlyItsRooms(t *testing.T) {
	ctx := context.Background()
	dir := t.TempDir()
	path := filepath.Join(dir, "chirper.db")
	s, err := Open(ctx, path)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = s.Close() })

	portal := id.RoomID("!portal.test:chirper.local")
	rooms := []*Room{
		{RoomID: testRoom, BridgeID: "demo", LoginID: "login-a", CreatedTS: 1},
		{RoomID: otherRoom, BridgeID: "demo", LoginID: "login-b", CreatedTS: 1},
		{RoomID: portal, BridgeID: "demo", CreatedTS: 1},
		{RoomID: "!elsewhere.test:chirper.local", BridgeID: "other", LoginID: "login-a", CreatedTS: 1},
	}
	for _, r := range rooms {
		if err := s.UpsertRoom(ctx, r); err != nil {
			t.Fatal(err)
		}
	}
	mediaA := putMediaFile(t, s, dir, "mediaA")
	mediaB := putMediaFile(t, s, dir, "mediaB")
	insertPic(t, s, "$a", testRoom, `{"url":"mxc://chirper.local/mediaA"}`)
	insertPic(t, s, "$b", otherRoom, `{"url":"mxc://chirper.local/mediaB"}`)
	insertPic(t, s, "$p", portal, `{"body":"from the portal"}`)
	if _, err := s.AddReaction(ctx, &Reaction{EventID: "$r", RoomID: testRoom, TargetEventID: "$a",
		Sender: "@alice:chirper.local", Key: "+1", TS: 11}); err != nil {
		t.Fatal(err)
	}
	if err := s.SetMembership(ctx, testRoom, mustUser("alice"), event.MembershipJoin); err != nil {
		t.Fatal(err)
	}

	if _, err := s.PurgeLogin(ctx, "demo", "login-a", []id.RoomID{portal}); err != nil {
		t.Fatalf("PurgeLogin: %v", err)
	}
	for _, gone := range []id.RoomID{testRoom, portal} {
		if _, err := s.GetRoom(ctx, gone); err != ErrNotFound {
			t.Errorf("room %s of the purged login survived: %v", gone, err)
		}
	}
	if fileExists(mediaA) {
		t.Error("media of the purged login survived")
	}
	if _, err := s.GetMedia(ctx, "mediaA"); err != ErrNotFound {
		t.Errorf("media row of the purged login survived: %v", err)
	}
	for _, kept := range []id.RoomID{otherRoom, "!elsewhere.test:chirper.local"} {
		if _, err := s.GetRoom(ctx, kept); err != nil {
			t.Errorf("room %s of another login was purged: %v", kept, err)
		}
	}
	if _, err := s.GetEvent(ctx, "$b"); err != nil {
		t.Errorf("event of another login was purged: %v", err)
	}
	if !fileExists(mediaB) {
		t.Error("media of another login was deleted")
	}
	if info, err := os.Stat(path + "-wal"); err == nil && info.Size() != 0 {
		t.Errorf("WAL not truncated after purge: %d bytes", info.Size())
	}
}

func TestPurgeLoginNeedsALoginID(t *testing.T) {
	if _, err := openTest(t).PurgeLogin(context.Background(), "demo", "", nil); err == nil {
		t.Fatal("purge without a login ID succeeded")
	}
}
