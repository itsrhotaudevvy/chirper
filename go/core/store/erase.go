package store

import (
	"context"
	"errors"
	"fmt"
	"io/fs"
	"os"

	"maunium.net/go/mautrix/id"
)

// ReleaseEventMedia unlinks the media of eventID and deletes the media rows and files that no event
// refers to any more. Call it once a redaction is final. An optimistic redaction can still be undone,
// and deleted media cannot come back.
func (s *Store) ReleaseEventMedia(ctx context.Context, eventID id.EventID) error {
	return s.releaseMediaWhere(ctx, "event_id = $1", eventID)
}

// releaseMediaWhere is ReleaseEventMedia for the links selected by where. The where clauses are
// constants in this package, never user input.
func (s *Store) releaseMediaWhere(ctx context.Context, where string, arg any) error {
	var paths []string
	err := s.db.DoTxn(ctx, nil, func(ctx context.Context) error {
		var err error
		paths, err = s.unlinkMedia(ctx, where, arg)
		return err
	})
	if err != nil {
		return err
	}
	return removeMediaFiles(paths)
}

// unlinkMedia removes the media links selected by where and deletes each media row that is left
// without links. It returns the paths of the deleted media, which the caller removes after commit.
// It must run inside a transaction.
func (s *Store) unlinkMedia(ctx context.Context, where string, arg any) ([]string, error) {
	mediaIDs, err := queryAll(ctx, s, func(row rowScanner) (string, error) {
		var mediaID string
		return mediaID, row.Scan(&mediaID)
	}, `SELECT DISTINCT media_id FROM event_media WHERE `+where, arg)
	if err != nil {
		return nil, fmt.Errorf("failed to find media: %w", err)
	}
	if _, err := s.db.Exec(ctx, `DELETE FROM event_media WHERE `+where, arg); err != nil {
		return nil, fmt.Errorf("failed to unlink media: %w", err)
	}
	var paths []string
	for _, mediaID := range mediaIDs {
		path, err := s.deleteUnreferencedMedia(ctx, mediaID)
		if err != nil {
			return nil, err
		}
		if path != "" {
			paths = append(paths, path)
		}
	}
	return paths, nil
}

// deleteUnreferencedMedia deletes the media row of mediaID when no event links to it, and returns
// the file path to remove. It returns "" when the media is still referenced or unknown.
func (s *Store) deleteUnreferencedMedia(ctx context.Context, mediaID string) (string, error) {
	links, err := queryOne(ctx, s, func(row rowScanner) (int64, error) {
		var n int64
		return n, row.Scan(&n)
	}, `SELECT COUNT(*) FROM event_media WHERE media_id = $1`, mediaID)
	if err != nil {
		return "", fmt.Errorf("failed to count media links: %w", err)
	}
	if links > 0 {
		return "", nil
	}
	m, err := s.GetMedia(ctx, mediaID)
	if errors.Is(err, ErrNotFound) {
		return "", nil
	} else if err != nil {
		return "", err
	}
	if _, err := s.db.Exec(ctx, `DELETE FROM media WHERE media_id = $1`, mediaID); err != nil {
		return "", fmt.Errorf("failed to delete media: %w", err)
	}
	return m.Path, nil
}

// removeMediaFiles deletes media files. A file that is already gone is not an error.
func removeMediaFiles(paths []string) error {
	var errs []error
	for _, p := range paths {
		if err := os.Remove(p); err != nil && !errors.Is(err, fs.ErrNotExist) {
			errs = append(errs, fmt.Errorf("failed to remove media file: %w", err))
		}
	}
	return errors.Join(errs...)
}

// eraseRoomContent deletes the events, reactions and members of roomID. It must run inside a
// transaction.
func (s *Store) eraseRoomContent(ctx context.Context, roomID id.RoomID) error {
	for _, query := range []string{
		`DELETE FROM event WHERE room_id = $1`,
		`DELETE FROM reaction WHERE room_id = $1`,
		`DELETE FROM member WHERE room_id = $1`,
	} {
		if _, err := s.db.Exec(ctx, query, roomID); err != nil {
			return fmt.Errorf("failed to erase room content: %w", err)
		}
	}
	return nil
}

// PurgeLogin erases the chats of loginID on network: their rooms, events, reactions, members and
// media, with the media files. Unlike DeleteRoom, the room rows are removed too. It returns the
// erased rooms.
//
// A room belongs to the login when its login_id is loginID. A room with no login_id belongs to it
// when it is one of portalRooms, the rooms of the login's portals in the bridge database. Rooms of
// other logins are kept. The WAL is then truncated, so that erased pages do not stay in the -wal file.
func (s *Store) PurgeLogin(ctx context.Context, network, loginID string, portalRooms []id.RoomID) ([]id.RoomID, error) {
	if loginID == "" {
		return nil, errors.New("store: purge needs a login ID")
	}
	rooms, err := s.loginRooms(ctx, network, loginID, portalRooms)
	if err != nil {
		return nil, err
	}
	var paths []string
	err = s.db.DoTxn(ctx, nil, func(ctx context.Context) error {
		for _, roomID := range rooms {
			released, err := s.unlinkMedia(ctx, "room_id = $1", roomID)
			if err != nil {
				return err
			}
			paths = append(paths, released...)
			if err := s.eraseRoomContent(ctx, roomID); err != nil {
				return err
			}
			if _, err := s.db.Exec(ctx, `DELETE FROM room WHERE room_id = $1`, roomID); err != nil {
				return fmt.Errorf("failed to delete room: %w", err)
			}
		}
		return nil
	})
	if err != nil {
		return nil, err
	}
	fileErr := removeMediaFiles(paths)
	if _, err := s.db.Exec(ctx, `PRAGMA wal_checkpoint(TRUNCATE)`); err != nil {
		return rooms, errors.Join(fileErr, fmt.Errorf("failed to checkpoint store: %w", err))
	}
	return rooms, fileErr
}

// loginRooms returns the rooms of network that belong to loginID (see PurgeLogin).
func (s *Store) loginRooms(ctx context.Context, network, loginID string, portalRooms []id.RoomID) ([]id.RoomID, error) {
	type roomLogin struct {
		roomID  id.RoomID
		loginID string
	}
	rows, err := queryAll(ctx, s, func(row rowScanner) (roomLogin, error) {
		var r roomLogin
		return r, row.Scan(&r.roomID, &r.loginID)
	}, `SELECT room_id, login_id FROM room WHERE bridge_id = $1`, network)
	if err != nil {
		return nil, fmt.Errorf("failed to list rooms: %w", err)
	}
	portal := make(map[id.RoomID]bool, len(portalRooms))
	for _, roomID := range portalRooms {
		portal[roomID] = true
	}
	var owned []id.RoomID
	for _, r := range rows {
		if r.loginID == loginID || (r.loginID == "" && portal[r.roomID]) {
			owned = append(owned, r.roomID)
		}
	}
	return owned, nil
}
