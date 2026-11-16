package store

import (
	"context"
	"fmt"

	"maunium.net/go/mautrix/id"
)

const eventColumns = `event_id, room_id, ord, sender, type, state_key, content_json, ts,
	COALESCE(txn_id, ''), reply_to, edit_content_json, edited_ts, redacted, status, error, part_index`

func scanEvent(row rowScanner) (*Event, error) {
	var e Event
	err := row.Scan(&e.EventID, &e.RoomID, &e.Order, &e.Sender, &e.Type, &e.StateKey, &e.ContentJSON,
		&e.TS, &e.TxnID, &e.ReplyTo, &e.EditJSON, &e.EditedTS, &e.Redacted, &e.Status, &e.Error, &e.PartIndex)
	if err != nil {
		return nil, err
	}
	return &e, nil
}

// orderBucket is the number of order slots per millisecond. ord = ts*orderBucket + n lets
// events with the same timestamp keep their insertion order and backfilled events sort before.
const orderBucket = 1000

func nullIfEmpty(s string) any {
	if s == "" {
		return nil
	}
	return s
}

// nextOrder returns an unused order key for an event with timestamp ts in roomID.
func (s *Store) nextOrder(ctx context.Context, roomID id.RoomID, ts int64) (int64, error) {
	base := ts * orderBucket
	maxOrd, err := queryOne(ctx, s, func(row rowScanner) (int64, error) {
		var m int64
		return m, row.Scan(&m)
	}, `SELECT COALESCE(MAX(ord), -1) FROM event WHERE room_id = $1 AND ord >= $2 AND ord < $3`,
		roomID, base, base+orderBucket)
	if err != nil {
		return 0, fmt.Errorf("failed to find next order: %w", err)
	}
	if maxOrd < base {
		return base, nil
	}
	return min(maxOrd+1, base+orderBucket-1), nil
}

// InsertEvent stores a visible timeline event. It is idempotent on EventID: inserted is false
// when the event already exists. On success e.Order is set to the assigned order key.
// The room's last event and activity are updated for non-state events.
func (s *Store) InsertEvent(ctx context.Context, e *Event) (inserted bool, err error) {
	if e.ReplyTo == "" {
		e.ReplyTo = replyToFromContent(e.ContentJSON)
	}
	err = s.db.DoTxn(ctx, nil, func(ctx context.Context) error {
		existing, err := queryOne(ctx, s, func(row rowScanner) (int64, error) {
			var o int64
			return o, row.Scan(&o)
		}, `SELECT ord FROM event WHERE event_id = $1`, e.EventID)
		if err == nil {
			e.Order = existing
			return nil
		} else if err != ErrNotFound {
			return err
		}
		e.Order, err = s.nextOrder(ctx, e.RoomID, e.TS)
		if err != nil {
			return err
		}
		res, err := s.db.Exec(ctx, `
			INSERT INTO event (event_id, room_id, ord, sender, type, state_key, content_json, ts, txn_id,
				reply_to, status, error, part_index)
			VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
			ON CONFLICT DO NOTHING`,
			e.EventID, e.RoomID, e.Order, e.Sender, e.Type, e.StateKey, e.ContentJSON, e.TS,
			nullIfEmpty(e.TxnID), e.ReplyTo, e.Status, e.Error, e.PartIndex)
		if err != nil {
			return fmt.Errorf("failed to insert event: %w", err)
		}
		if n, _ := res.RowsAffected(); n == 0 {
			// A row with the same transaction ID already exists under another event ID.
			return nil
		}
		inserted = true
		if err := linkMedia(ctx, s.db, e.EventID, e.RoomID, e.ContentJSON); err != nil {
			return err
		}
		return s.touchRoom(ctx, e)
	})
	return inserted, err
}

func (s *Store) touchRoom(ctx context.Context, e *Event) error {
	if e.StateKey != nil {
		_, err := s.db.Exec(ctx, `UPDATE room SET last_activity_ts = MAX(last_activity_ts, $2) WHERE room_id = $1`,
			e.RoomID, e.TS)
		return err
	}
	// The preview follows the newest event by timestamp, so an older event inserted later
	// (backfill) does not replace it. SQLite evaluates the SET expressions on the old row.
	_, err := s.db.Exec(ctx, `
		UPDATE room SET
			last_event_id = CASE WHEN $2 >= last_activity_ts THEN $3 ELSE last_event_id END,
			last_activity_ts = MAX(last_activity_ts, $2)
		WHERE room_id = $1`, e.RoomID, e.TS, e.EventID)
	if err != nil {
		return fmt.Errorf("failed to update room activity: %w", err)
	}
	return nil
}

// GetEvent returns a stored timeline event, or ErrNotFound.
func (s *Store) GetEvent(ctx context.Context, eventID id.EventID) (*Event, error) {
	return queryOne(ctx, s, scanEvent, `SELECT `+eventColumns+` FROM event WHERE event_id = $1`, eventID)
}

// ApplyEdit stores newContentJSON (the m.new_content of an edit) on the target event.
func (s *Store) ApplyEdit(ctx context.Context, roomID id.RoomID, targetEventID id.EventID, newContentJSON string, ts int64) error {
	res, err := s.db.Exec(ctx, `
		UPDATE event SET edit_content_json = $3, edited_ts = $4
		WHERE room_id = $1 AND event_id = $2 AND redacted = false`,
		roomID, targetEventID, newContentJSON, ts)
	if err != nil {
		return fmt.Errorf("failed to apply edit: %w", err)
	}
	if n, _ := res.RowsAffected(); n == 0 {
		return ErrNotFound
	}
	return nil
}

// Redact removes the content of a message or, when eventID is a stored reaction, deletes the
// reaction. It returns ErrNotFound when eventID is neither.
func (s *Store) Redact(ctx context.Context, eventID id.EventID) (Redaction, error) {
	var out Redaction
	err := s.db.DoTxn(ctx, nil, func(ctx context.Context) error {
		reaction, err := s.RemoveReactionByEventID(ctx, eventID)
		if err == nil {
			out = Redaction{RoomID: reaction.RoomID, EventID: eventID, TargetEventID: reaction.TargetEventID, IsReaction: true}
			return nil
		} else if err != ErrNotFound {
			return err
		}
		ev, err := s.GetEvent(ctx, eventID)
		if err != nil {
			return err
		}
		_, err = s.db.Exec(ctx, `
			UPDATE event SET redacted = true, content_json = '{}', edit_content_json = NULL, error = ''
			WHERE event_id = $1`, eventID)
		if err != nil {
			return fmt.Errorf("failed to redact event: %w", err)
		}
		out = Redaction{RoomID: ev.RoomID, EventID: eventID, TargetEventID: eventID}
		return nil
	})
	return out, err
}

// RemoveReactionByEventID deletes the reaction stored under eventID and returns it, or
// ErrNotFound.
func (s *Store) RemoveReactionByEventID(ctx context.Context, eventID id.EventID) (Reaction, error) {
	r, err := queryOne(ctx, s, func(row rowScanner) (Reaction, error) {
		var r Reaction
		err := row.Scan(&r.RoomID, &r.TargetEventID)
		return r, err
	}, `SELECT room_id, target_event_id FROM reaction WHERE event_id = $1`, eventID)
	if err != nil {
		return Reaction{}, err
	}
	if _, err := s.db.Exec(ctx, `DELETE FROM reaction WHERE event_id = $1`, eventID); err != nil {
		return Reaction{}, fmt.Errorf("failed to delete reaction: %w", err)
	}
	r.EventID = eventID
	return r, nil
}

// AddReaction stores a reaction. A sender's reaction with the same key on the same target is
// stored only once; inserted is false in that case or when the reaction event already exists.
func (s *Store) AddReaction(ctx context.Context, r *Reaction) (inserted bool, err error) {
	res, err := s.db.Exec(ctx, `
		INSERT INTO reaction (event_id, room_id, target_event_id, sender, key, ts)
		SELECT $1, $2, $3, $4, $5, $6
		WHERE NOT EXISTS (
			SELECT 1 FROM reaction WHERE target_event_id = $3 AND sender = $4 AND key = $5
		) ON CONFLICT (event_id) DO NOTHING`,
		r.EventID, r.RoomID, r.TargetEventID, r.Sender, r.Key, r.TS)
	if err != nil {
		return false, fmt.Errorf("failed to add reaction: %w", err)
	}
	n, _ := res.RowsAffected()
	return n > 0, nil
}

// SetEventStatus updates the send status of an outgoing event. Unknown events are ignored, since
// not every acknowledged Matrix event (e.g. reactions) is stored as a timeline event.
func (s *Store) SetEventStatus(ctx context.Context, eventID id.EventID, status, errText string) error {
	_, err := s.db.Exec(ctx, `UPDATE event SET status = $2, error = $3 WHERE event_id = $1`, eventID, status, errText)
	if err != nil {
		return fmt.Errorf("failed to set event status: %w", err)
	}
	return nil
}

// FindReactions returns the stored reactions of sender on target. Keys are returned as stored.
func (s *Store) FindReactions(ctx context.Context, target id.EventID, sender id.UserID) ([]Reaction, error) {
	rows, err := queryAll(ctx, s, func(row rowScanner) (Reaction, error) {
		var r Reaction
		err := row.Scan(&r.EventID, &r.RoomID, &r.TargetEventID, &r.Sender, &r.Key, &r.TS)
		return r, err
	}, `SELECT event_id, room_id, target_event_id, sender, key, ts FROM reaction
		WHERE target_event_id = $1 AND sender = $2 ORDER BY ts, event_id`, target, sender)
	if err != nil {
		return nil, fmt.Errorf("failed to find reactions: %w", err)
	}
	return rows, nil
}

// RestoreEvent writes the content columns of snap back to the stored event. It undoes an optimistic
// edit or redaction that the network rejected.
func (s *Store) RestoreEvent(ctx context.Context, snap *Event) error {
	_, err := s.db.Exec(ctx, `
		UPDATE event SET content_json = $2, edit_content_json = $3, edited_ts = $4, redacted = $5
		WHERE event_id = $1`,
		snap.EventID, snap.ContentJSON, snap.EditJSON, snap.EditedTS, snap.Redacted)
	if err != nil {
		return fmt.Errorf("failed to restore event: %w", err)
	}
	return nil
}
