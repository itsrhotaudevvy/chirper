package store

import (
	"context"
	"fmt"
	"strings"

	"maunium.net/go/mautrix/event"
	"maunium.net/go/mautrix/id"
)

var roomColumnNames = []string{"room_id", "bridge_id", "login_id", "portal_id", "name", "topic", "avatar_url",
	"is_dm", "is_space", "dm_user_id", "created_ts", "last_activity_ts", "last_event_id", "unread_count",
	"mention_count", "read_up_to_order", "pinned", "archived", "muted_until", "tags_json", "deleted", "parent_id"}

// roomColumns lists the room columns for queries on the room table alone.
var roomColumns = strings.Join(roomColumnNames, ", ")

// qualifiedRoomColumns lists the room columns prefixed with the table name, for joined queries.
var qualifiedRoomColumns = func() string {
	cols := make([]string, len(roomColumnNames))
	for i, c := range roomColumnNames {
		cols[i] = "room." + c
	}
	return strings.Join(cols, ", ")
}()

// roomDest returns the scan destinations for roomColumns, in the same order.
func roomDest(r *Room) []any {
	return []any{&r.RoomID, &r.BridgeID, &r.LoginID, &r.PortalID, &r.Name, &r.Topic, &r.AvatarURL,
		&r.IsDM, &r.IsSpace, &r.DMUserID, &r.CreatedTS, &r.LastActivityTS, &r.LastEventID,
		&r.UnreadCount, &r.MentionCount, &r.ReadUpToOrder, &r.Pinned, &r.Archived, &r.MutedUntil,
		&r.TagsJSON, &r.Deleted, &r.ParentID}
}

func scanRoom(row rowScanner) (*Room, error) {
	var r Room
	if err := row.Scan(roomDest(&r)...); err != nil {
		return nil, err
	}
	return &r, nil
}

// UpsertRoom creates the room or updates its identity columns (bridge, login, portal, DM and
// space flags). Name, topic, avatar and user flags are left alone; use SetRoomFields for those.
// A non-empty LoginID/PortalID is kept when the new value is empty.
func (s *Store) UpsertRoom(ctx context.Context, r *Room) error {
	_, err := s.db.Exec(ctx, `
		INSERT INTO room (room_id, bridge_id, login_id, portal_id, is_dm, is_space, dm_user_id, created_ts, deleted, parent_id)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $8, false, $9)
		ON CONFLICT (room_id) DO UPDATE SET
			bridge_id = excluded.bridge_id,
			login_id = COALESCE(NULLIF(excluded.login_id, ''), room.login_id),
			portal_id = COALESCE(NULLIF(excluded.portal_id, ''), room.portal_id),
			is_dm = excluded.is_dm,
			is_space = excluded.is_space,
			dm_user_id = excluded.dm_user_id,
			deleted = false,
			parent_id = COALESCE(NULLIF(excluded.parent_id, ''), room.parent_id)`,
		r.RoomID, r.BridgeID, r.LoginID, r.PortalID, r.IsDM, r.IsSpace, r.DMUserID, r.CreatedTS, r.ParentID)
	if err != nil {
		return fmt.Errorf("failed to upsert room: %w", err)
	}
	return nil
}

// GetRoom returns the room row, including deleted rooms.
func (s *Store) GetRoom(ctx context.Context, roomID id.RoomID) (*Room, error) {
	return queryOne(ctx, s, scanRoom, `SELECT `+roomColumns+` FROM room WHERE room_id = $1`, roomID)
}

// SetRoomFields applies the non-nil fields of f to the room.
func (s *Store) SetRoomFields(ctx context.Context, roomID id.RoomID, f RoomFields) error {
	var sets []string
	var args []any
	add := func(column string, value any) {
		args = append(args, value)
		sets = append(sets, fmt.Sprintf("%s = $%d", column, len(args)))
	}
	if f.LoginID != nil {
		add("login_id", *f.LoginID)
	}
	if f.PortalID != nil {
		add("portal_id", *f.PortalID)
	}
	if f.Name != nil {
		add("name", *f.Name)
	}
	if f.Topic != nil {
		add("topic", *f.Topic)
	}
	if f.AvatarURL != nil {
		add("avatar_url", *f.AvatarURL)
	}
	if f.Pinned != nil {
		add("pinned", *f.Pinned)
	}
	if f.Archived != nil {
		add("archived", *f.Archived)
	}
	if f.MutedUntil != nil {
		add("muted_until", *f.MutedUntil)
	}
	if f.TagsJSON != nil {
		add("tags_json", *f.TagsJSON)
	}
	if f.ParentID != nil {
		add("parent_id", *f.ParentID)
	}
	if len(sets) == 0 {
		return nil
	}
	args = append(args, roomID)
	_, err := s.db.Exec(ctx,
		fmt.Sprintf("UPDATE room SET %s WHERE room_id = $%d", strings.Join(sets, ", "), len(args)), args...)
	if err != nil {
		return fmt.Errorf("failed to update room fields: %w", err)
	}
	return nil
}

// DeleteRoom marks the room as deleted and erases its content: messages, reactions, members and
// the media files they refer to (unless another event still refers to them). The room's name, topic
// and avatar are cleared. The room row remains, hidden from listings, so that a later rejoin can
// reuse it.
func (s *Store) DeleteRoom(ctx context.Context, roomID id.RoomID) error {
	var paths []string
	err := s.db.DoTxn(ctx, nil, func(ctx context.Context) error {
		var err error
		if paths, err = s.unlinkMedia(ctx, "room_id = $1", roomID); err != nil {
			return err
		}
		if err := s.eraseRoomContent(ctx, roomID); err != nil {
			return err
		}
		_, err = s.db.Exec(ctx, `
			UPDATE room SET deleted = true, name = '', topic = '', avatar_url = '', last_event_id = '',
				unread_count = 0, mention_count = 0, read_up_to_order = 0 WHERE room_id = $1`, roomID)
		if err != nil {
			return fmt.Errorf("failed to delete room: %w", err)
		}
		return nil
	})
	if err != nil {
		return err
	}
	return removeMediaFiles(paths)
}

// MarkReadUpTo moves the read marker to eventID and recomputes the unread count.
func (s *Store) MarkReadUpTo(ctx context.Context, roomID id.RoomID, eventID id.EventID) error {
	return s.db.DoTxn(ctx, nil, func(ctx context.Context) error {
		ord, err := s.eventOrder(ctx, roomID, eventID)
		if err != nil {
			return err
		}
		_, err = s.db.Exec(ctx, `
			UPDATE room SET read_up_to_order = $2, unread_count = (
				SELECT COUNT(*) FROM event
				WHERE event.room_id = $1 AND event.ord > $2 AND event.sender <> $3
				  AND event.state_key IS NULL AND event.redacted = false
			) WHERE room_id = $1`, roomID, ord, SelfUserID)
		if err != nil {
			return fmt.Errorf("failed to update read marker: %w", err)
		}
		return nil
	})
}

func (s *Store) eventOrder(ctx context.Context, roomID id.RoomID, eventID id.EventID) (int64, error) {
	ord, err := queryOne(ctx, s, func(row rowScanner) (int64, error) {
		var o int64
		return o, row.Scan(&o)
	}, `SELECT ord FROM event WHERE room_id = $1 AND event_id = $2`, roomID, eventID)
	if err != nil {
		return 0, fmt.Errorf("failed to find event %s: %w", eventID, err)
	}
	return ord, nil
}

// IncrementUnread counts an inserted incoming message as unread if it is newer than the room's
// read marker. Messages from the local user and state events are never counted.
func (s *Store) IncrementUnread(ctx context.Context, eventID id.EventID) error {
	ev, err := s.GetEvent(ctx, eventID)
	if err != nil {
		return err
	}
	if ev.Sender == SelfUserID || ev.StateKey != nil {
		return nil
	}
	_, err = s.db.Exec(ctx, `
		UPDATE room SET unread_count = unread_count + 1
		WHERE room_id = $1 AND read_up_to_order < $2`, ev.RoomID, ev.Order)
	if err != nil {
		return fmt.Errorf("failed to increment unread count: %w", err)
	}
	return nil
}

// MarkUnread sets the room's unread flag. Marking unread makes the count at least 1; marking
// read moves the read marker to the newest event.
func (s *Store) MarkUnread(ctx context.Context, roomID id.RoomID, unread bool) error {
	if !unread {
		room, err := s.GetRoom(ctx, roomID)
		if err != nil {
			return err
		}
		if room.LastEventID == "" {
			return nil
		}
		return s.MarkReadUpTo(ctx, roomID, room.LastEventID)
	}
	_, err := s.db.Exec(ctx, `UPDATE room SET unread_count = MAX(unread_count, 1) WHERE room_id = $1`, roomID)
	if err != nil {
		return fmt.Errorf("failed to mark room unread: %w", err)
	}
	return nil
}

// chatSelect reads rooms with their chat summary in one query: the joined member count and the
// last visible event, with the sender's display name (member name, then profile name).
var chatSelect = "SELECT " + qualifiedRoomColumns + `,
	(SELECT COUNT(*) FROM member WHERE member.room_id = room.room_id AND member.membership = 'join'),
	ev.event_id, ev.sender, ev.type, ev.state_key, ev.content_json, ev.edit_content_json,
	COALESCE(NULLIF(em.displayname, ''), NULLIF(pr.displayname, '')),
	COALESCE((SELECT name FROM room p WHERE (p.room_id = room.parent_id OR (p.portal_id = room.parent_id AND p.bridge_id = room.bridge_id AND p.is_space = true)) AND p.deleted = false LIMIT 1), '')
	FROM room
	LEFT JOIN event ev ON ev.event_id = room.last_event_id
	LEFT JOIN member em ON em.room_id = ev.room_id AND em.user_id = ev.sender
	LEFT JOIN profile pr ON pr.user_id = ev.sender`

// scanChat reads one row of chatSelect.
func scanChat(row rowScanner) (Chat, error) {
	var (
		room                               Room
		memberCount                        int
		evID, evSender, evType, evStateKey *string
		evContent, evEdit, senderName      *string
		serverName                         *string
	)
	dest := append(roomDest(&room), &memberCount, &evID, &evSender, &evType, &evStateKey, &evContent, &evEdit, &senderName, &serverName)
	if err := row.Scan(dest...); err != nil {
		return Chat{}, err
	}
	sName := deref(serverName)
	isChannel := room.ParentID != ""
	if isChannel && sName == "" && room.BridgeID != "" {
		sName = strings.ToUpper(room.BridgeID[:1]) + room.BridgeID[1:] + " Server"
	}
	chat := Chat{
		RoomID:       room.RoomID,
		Network:      room.BridgeID,
		LoginID:      room.LoginID,
		Name:         room.Name,
		AvatarURL:    room.AvatarURL,
		Topic:        room.Topic,
		IsDM:         room.IsDM,
		IsGroup:      !room.IsDM && !room.IsSpace && !isChannel,
		IsSpace:      room.IsSpace,
		MemberCount:  memberCount,
		LastActivity: room.LastActivityTS,
		UnreadCount:  room.UnreadCount,
		MentionCount: room.MentionCount,
		Pinned:       room.Pinned,
		Archived:     room.Archived,
		MutedUntil:   room.MutedUntil,
		Typing:       []string{},
		ParentID:     room.ParentID,
		ServerName:   sName,
		IsChannel:    isChannel,
	}
	if evID != nil {
		ev := &Event{
			EventID:     id.EventID(*evID),
			RoomID:      room.RoomID,
			Sender:      id.UserID(deref(evSender)),
			Type:        deref(evType),
			StateKey:    evStateKey,
			ContentJSON: deref(evContent),
			EditJSON:    evEdit,
		}
		preview, err := previewOf(ev, deref(senderName))
		if err != nil {
			return Chat{}, err
		}
		chat.Preview = preview
	}
	return chat, nil
}

func deref(s *string) string {
	if s == nil {
		return ""
	}
	return *s
}

// GetChat builds the JSON chat for a room, or returns ErrNotFound. Typing is always empty here; the engine fills it.
func (s *Store) GetChat(ctx context.Context, roomID id.RoomID) (*Chat, error) {
	chat, err := queryOne(ctx, s, scanChat, chatSelect+` WHERE room.room_id = $1`, roomID)
	if err != nil {
		return nil, err
	}
	return &chat, nil
}

// ListChats returns the chats matching filter, pinned first, then most recently active.
func (s *Store) ListChats(ctx context.Context, filter ChatFilter) ([]Chat, error) {
	query := chatSelect + ` WHERE room.deleted = false AND room.is_space = false AND room.archived = $1`
	args := []any{filter.Archived}
	if filter.Network != "" {
		args = append(args, filter.Network)
		query += fmt.Sprintf(" AND room.bridge_id = $%d", len(args))
	}
	if filter.DirectOnly {
		query += " AND (room.parent_id = '' OR room.is_dm = true)"
	} else if filter.ServersOnly {
		query += " AND room.parent_id <> '' AND room.is_dm = false"
	}
	if filter.UnreadOnly {
		query += " AND room.unread_count > 0"
	}
	if filter.Query != "" {
		args = append(args, likePattern(filter.Query))
		query += fmt.Sprintf(` AND (room.name LIKE $%d ESCAPE '\' OR (SELECT name FROM room p WHERE (p.room_id = room.parent_id OR (p.portal_id = room.parent_id AND p.bridge_id = room.bridge_id AND p.is_space = true)) AND p.deleted = false LIMIT 1) LIKE $%d ESCAPE '\')`, len(args), len(args))
	}
	query += " ORDER BY room.pinned DESC, room.last_activity_ts DESC, room.room_id"
	chats, err := queryAll(ctx, s, scanChat, query, args...)
	if err != nil {
		return nil, fmt.Errorf("failed to list chats: %w", err)
	}
	if chats == nil {
		chats = []Chat{}
	}
	return chats, nil
}

// FindSpaceRoom finds a space room by portal ID and bridge ID.
func (s *Store) FindSpaceRoom(ctx context.Context, bridgeID, portalID string) (*Room, error) {
	return queryOne(ctx, s, scanRoom,
		`SELECT `+roomColumns+` FROM room WHERE bridge_id = $1 AND portal_id = $2 AND is_space = true AND deleted = false LIMIT 1`,
		bridgeID, portalID)
}

// SetMembership sets only the membership of a user in a room, creating the row if needed.
func (s *Store) SetMembership(ctx context.Context, roomID id.RoomID, userID id.UserID, membership event.Membership) error {
	_, err := s.db.Exec(ctx, `
		INSERT INTO member (room_id, user_id, displayname, avatar_url, membership)
		VALUES ($1, $2, '', '', $3)
		ON CONFLICT (room_id, user_id) DO UPDATE SET membership = excluded.membership`,
		roomID, userID, string(membership))
	if err != nil {
		return fmt.Errorf("failed to set membership: %w", err)
	}
	return nil
}

// UpsertMember stores the full member row.
func (s *Store) UpsertMember(ctx context.Context, m *Member) error {
	_, err := s.db.Exec(ctx, `
		INSERT INTO member (room_id, user_id, displayname, avatar_url, membership)
		VALUES ($1, $2, $3, $4, $5)
		ON CONFLICT (room_id, user_id) DO UPDATE SET
			displayname = excluded.displayname,
			avatar_url = excluded.avatar_url,
			membership = excluded.membership`,
		m.RoomID, m.UserID, m.Displayname, m.AvatarURL, string(m.Membership))
	if err != nil {
		return fmt.Errorf("failed to upsert member: %w", err)
	}
	return nil
}

func scanMember(row rowScanner) (Member, error) {
	var m Member
	err := row.Scan(&m.RoomID, &m.UserID, &m.Displayname, &m.AvatarURL, &m.Membership)
	return m, err
}

// GetMember returns the member row, or ErrNotFound.
func (s *Store) GetMember(ctx context.Context, roomID id.RoomID, userID id.UserID) (*Member, error) {
	m, err := queryOne(ctx, s, scanMember,
		`SELECT room_id, user_id, displayname, avatar_url, membership FROM member WHERE room_id = $1 AND user_id = $2`,
		roomID, userID)
	if err != nil {
		return nil, err
	}
	return &m, nil
}

// ListMembers returns all member rows of a room, including members who have left.
func (s *Store) ListMembers(ctx context.Context, roomID id.RoomID) ([]Member, error) {
	return queryAll(ctx, s, scanMember,
		`SELECT room_id, user_id, displayname, avatar_url, membership FROM member WHERE room_id = $1 ORDER BY user_id`,
		roomID)
}

// DisplayName returns the name shown for userID in roomID: the member name, else the global
// profile name, else the user ID.
func (s *Store) DisplayName(ctx context.Context, roomID id.RoomID, userID id.UserID) (string, error) {
	name, _, err := s.senderInfo(ctx, roomID, userID)
	return name, err
}
