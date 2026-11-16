package store

import (
	"context"
	"encoding/json"
	"fmt"

	"go.mau.fi/util/dbutil"
	"maunium.net/go/mautrix/id"
)

// schemaV3 links each event to the local media that its content refers to. The links let redaction,
// room deletion and logout find the media files that no event refers to any more. Links go with
// their event.
const schemaV3 = `
CREATE TABLE event_media (
	event_id TEXT NOT NULL REFERENCES event(event_id) ON DELETE CASCADE,
	room_id  TEXT NOT NULL,
	media_id TEXT NOT NULL,
	PRIMARY KEY (event_id, media_id)
);
CREATE INDEX event_media_media_idx ON event_media(media_id);
CREATE INDEX event_media_room_idx ON event_media(room_id);
`

// migrateMediaLinks creates the event_media table and links the events that an earlier schema stored
// without links.
func migrateMediaLinks(ctx context.Context, db *dbutil.Database) error {
	if _, err := db.Exec(ctx, schemaV3); err != nil {
		return err
	}
	type storedEvent struct {
		eventID, roomID, content string
	}
	rows, err := db.Query(ctx, `SELECT event_id, room_id, content_json FROM event WHERE content_json LIKE '%mxc://%'`)
	if err != nil {
		return fmt.Errorf("failed to read events: %w", err)
	}
	// The single connection cannot write while the cursor is open, so the rows are read first.
	var events []storedEvent
	for rows.Next() {
		var e storedEvent
		if err := rows.Scan(&e.eventID, &e.roomID, &e.content); err != nil {
			_ = rows.Close()
			return err
		}
		events = append(events, e)
	}
	if err := rows.Err(); err != nil {
		_ = rows.Close()
		return err
	}
	if err := rows.Close(); err != nil {
		return err
	}
	for _, e := range events {
		if err := linkMedia(ctx, db, id.EventID(e.eventID), id.RoomID(e.roomID), e.content); err != nil {
			return err
		}
	}
	return nil
}

// mediaRefs are the parts of a message's content that name media: the file and its thumbnail.
type mediaRefs struct {
	URL  string `json:"url"`
	Info struct {
		ThumbnailURL string `json:"thumbnail_url"`
	} `json:"info"`
}

// linkMedia records that eventID refers to the local media named in its content. Media of other
// servers is not stored here and is not linked. Content that is not a JSON object names no media.
func linkMedia(ctx context.Context, db *dbutil.Database, eventID id.EventID, roomID id.RoomID, contentJSON string) error {
	var refs mediaRefs
	if err := json.Unmarshal([]byte(contentJSON), &refs); err != nil {
		return nil
	}
	for _, uri := range []string{refs.URL, refs.Info.ThumbnailURL} {
		mediaID, ok := localMediaID(uri)
		if !ok {
			continue
		}
		_, err := db.Exec(ctx, `
			INSERT INTO event_media (event_id, room_id, media_id) VALUES ($1, $2, $3)
			ON CONFLICT DO NOTHING`, eventID, roomID, mediaID)
		if err != nil {
			return fmt.Errorf("failed to link media: %w", err)
		}
	}
	return nil
}

// localMediaID returns the media ID of uri when it is an mxc URI on this server.
func localMediaID(uri string) (string, bool) {
	if uri == "" {
		return "", false
	}
	parsed, err := id.ContentURIString(uri).Parse()
	if err != nil || parsed.Homeserver != ServerName {
		return "", false
	}
	return parsed.FileID, true
}
