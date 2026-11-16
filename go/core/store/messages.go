package store

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"unicode/utf8"

	"maunium.net/go/mautrix/event"
	"maunium.net/go/mautrix/id"
)

const (
	typeMessage  = "m.room.message"
	typeSticker  = "m.sticker"
	previewRunes = 140
)

// replyToFromContent extracts the m.in_reply_to target of an event content, if any.
func replyToFromContent(contentJSON string) id.EventID {
	var c struct {
		RelatesTo *event.RelatesTo `json:"m.relates_to"`
	}
	if json.Unmarshal([]byte(contentJSON), &c) != nil {
		return ""
	}
	return c.RelatesTo.GetReplyTo()
}

// described is the display form of an event's content.
type described struct {
	Type          string
	Body          string
	FormattedBody string
	Media         *MessageMedia
	Edited        bool
}

// describe maps the stored content of a message event to its display type and body. The edit,
// if any, replaces the body. Redacted and non-message events are described without content.
func describe(e *Event) (described, error) {
	if e.Type == typeSticker {
		return describeMedia("sticker", e)
	}
	if e.Type != typeMessage {
		return described{Type: "unsupported"}, nil
	}
	var c event.MessageEventContent
	if err := json.Unmarshal([]byte(e.ContentJSON), &c); err != nil {
		return described{}, fmt.Errorf("failed to parse message content of %s: %w", e.EventID, err)
	}
	var out described
	switch c.MsgType {
	case event.MsgText:
		out.Type = "text"
	case event.MsgNotice:
		out.Type = "notice"
	case event.MsgEmote:
		out.Type = "emote"
	case event.MsgImage:
		return describeMedia("image", e)
	case event.MsgVideo:
		return describeMedia("video", e)
	case event.MsgAudio:
		if c.MSC3245Voice != nil {
			return describeMedia("voice", e)
		}
		return describeMedia("audio", e)
	case event.MsgFile:
		return describeMedia("file", e)
	case event.MsgLocation:
		out.Type = "location"
	default:
		return described{Type: "unsupported"}, nil
	}
	out.Body = c.Body
	out.FormattedBody = c.FormattedBody
	if e.EditJSON != nil {
		var edit event.MessageEventContent
		if err := json.Unmarshal([]byte(*e.EditJSON), &edit); err != nil {
			return described{}, fmt.Errorf("failed to parse edit content of %s: %w", e.EventID, err)
		}
		out.Body = edit.Body
		out.FormattedBody = edit.FormattedBody
		out.Edited = true
	}
	return out, nil
}

func describeMedia(kind string, e *Event) (described, error) {
	var c event.MessageEventContent
	if err := json.Unmarshal([]byte(e.ContentJSON), &c); err != nil {
		return described{}, fmt.Errorf("failed to parse media content of %s: %w", e.EventID, err)
	}
	media := &MessageMedia{FileName: c.FileName}
	if media.FileName == "" && kind == "file" {
		media.FileName = c.Body
	}
	media.URL = string(c.URL)
	if c.File != nil {
		media.URL = string(c.File.URL)
	}
	if info := c.Info; info != nil {
		media.Mime = info.MimeType
		media.Size = info.Size
		media.Width = info.Width
		media.Height = info.Height
		media.DurationMs = info.Duration
		media.ThumbnailURL = string(info.ThumbnailURL)
		media.Blurhash = info.Blurhash
	}
	return described{Type: kind, Body: c.Body, Media: media}, nil
}

// previewText is the chat-list summary of a described event.
func previewText(d described) string {
	var text string
	switch d.Type {
	case "text", "notice", "emote", "location":
		text = d.Body
	case "image":
		text = "📷 Photo"
	case "video":
		text = "🎥 Video"
	case "voice":
		text = "🎤 Voice message"
	case "audio":
		text = "🎵 Audio"
	case "file":
		text = "📎 " + d.Media.FileName
	case "sticker":
		text = "Sticker"
	}
	if utf8.RuneCountInString(text) > previewRunes {
		text = string([]rune(text)[:previewRunes])
	}
	return text
}

// senderKey identifies a sender within a room.
type senderKey struct {
	roomID id.RoomID
	userID id.UserID
}

// senderName is a resolved display name and avatar.
type senderName struct {
	name   string
	avatar id.ContentURIString
}

// readCache memoises sender lookups for the duration of one read, so that a page of messages
// from a few senders costs one lookup per sender rather than one per message.
type readCache struct {
	senders map[senderKey]senderName
}

func newReadCache() *readCache {
	return &readCache{senders: map[senderKey]senderName{}}
}

// sender is senderInfo through rc.
func (s *Store) sender(ctx context.Context, rc *readCache, roomID id.RoomID, userID id.UserID) (string, id.ContentURIString, error) {
	key := senderKey{roomID, userID}
	if v, ok := rc.senders[key]; ok {
		return v.name, v.avatar, nil
	}
	name, avatar, err := s.senderInfo(ctx, roomID, userID)
	if err != nil {
		return "", "", err
	}
	rc.senders[key] = senderName{name, avatar}
	return name, avatar, nil
}

// senderInfo resolves the display name and avatar of a user in a room: the room member row,
// falling back to the global profile, falling back to the user ID.
func (s *Store) senderInfo(ctx context.Context, roomID id.RoomID, userID id.UserID) (name string, avatar id.ContentURIString, err error) {
	member, err := s.GetMember(ctx, roomID, userID)
	if err != nil && err != ErrNotFound {
		return "", "", err
	}
	if member != nil {
		name, avatar = member.Displayname, member.AvatarURL
	}
	if name == "" || avatar == "" {
		profile, err := s.GetProfile(ctx, userID)
		if err != nil && err != ErrNotFound {
			return "", "", err
		}
		if profile != nil {
			name = firstNonEmpty(name, profile.Displayname)
			avatar = firstNonEmptyMXC(avatar, profile.AvatarURL)
		}
	}
	return firstNonEmpty(name, string(userID)), avatar, nil
}

func firstNonEmpty(values ...string) string {
	for _, v := range values {
		if v != "" {
			return v
		}
	}
	return ""
}

func firstNonEmptyMXC(values ...id.ContentURIString) id.ContentURIString {
	for _, v := range values {
		if v != "" {
			return v
		}
	}
	return ""
}

// GetProfile returns the global profile of a user, or ErrNotFound.
func (s *Store) GetProfile(ctx context.Context, userID id.UserID) (*Profile, error) {
	return queryOne(ctx, s, func(row rowScanner) (*Profile, error) {
		var p Profile
		err := row.Scan(&p.UserID, &p.Displayname, &p.AvatarURL, &p.ExtraJSON)
		return &p, err
	}, `SELECT user_id, displayname, avatar_url, extra_json FROM profile WHERE user_id = $1`, userID)
}

// SetProfile stores the global profile of a user and copies its name and avatar onto the
// user's member rows, since the displayed name of a ghost is global.
func (s *Store) SetProfile(ctx context.Context, p *Profile) error {
	return s.db.DoTxn(ctx, nil, func(ctx context.Context) error {
		_, err := s.db.Exec(ctx, `
			INSERT INTO profile (user_id, displayname, avatar_url, extra_json) VALUES ($1, $2, $3, $4)
			ON CONFLICT (user_id) DO UPDATE SET
				displayname = excluded.displayname,
				avatar_url = excluded.avatar_url,
				extra_json = excluded.extra_json`,
			p.UserID, p.Displayname, p.AvatarURL, p.ExtraJSON)
		if err != nil {
			return fmt.Errorf("failed to set profile: %w", err)
		}
		_, err = s.db.Exec(ctx, `UPDATE member SET displayname = $2, avatar_url = $3 WHERE user_id = $1`,
			p.UserID, p.Displayname, p.AvatarURL)
		if err != nil {
			return fmt.Errorf("failed to propagate profile to members: %w", err)
		}
		return nil
	})
}

// stateBody is the human-readable summary of a state event.
func (s *Store) stateBody(ctx context.Context, rc *readCache, e *Event) (string, error) {
	sender, _, err := s.sender(ctx, rc, e.RoomID, e.Sender)
	if err != nil {
		return "", err
	}
	switch e.Type {
	case "m.room.name":
		var c event.RoomNameEventContent
		if err := json.Unmarshal([]byte(e.ContentJSON), &c); err != nil {
			return "", err
		}
		if c.Name == "" {
			return sender + " removed the group name", nil
		}
		return sender + " changed the group name to " + c.Name, nil
	case "m.room.topic":
		var c event.TopicEventContent
		if err := json.Unmarshal([]byte(e.ContentJSON), &c); err != nil {
			return "", err
		}
		if c.Topic == "" {
			return sender + " removed the topic", nil
		}
		return sender + " changed the topic to " + c.Topic, nil
	case "m.room.avatar":
		return sender + " changed the group photo", nil
	case "m.room.member":
		var c event.MemberEventContent
		if err := json.Unmarshal([]byte(e.ContentJSON), &c); err != nil {
			return "", err
		}
		target, _, err := s.sender(ctx, rc, e.RoomID, id.UserID(*e.StateKey))
		if err != nil {
			return "", err
		}
		return memberBody(c.Membership, sender, target, *e.StateKey == string(e.Sender)), nil
	}
	return sender + " updated the room", nil
}

func memberBody(membership event.Membership, sender, target string, self bool) string {
	switch membership {
	case event.MembershipJoin:
		return target + " joined"
	case event.MembershipLeave:
		if self {
			return target + " left"
		}
		return sender + " removed " + target
	case event.MembershipInvite:
		return sender + " invited " + target
	case event.MembershipBan:
		return sender + " banned " + target
	}
	return sender + " changed membership of " + target
}

// reactionsOf reads the stored reactions of the target events in one query, grouped by target and
// kept in stored order.
func (s *Store) reactionsOf(ctx context.Context, targets []id.EventID) (map[id.EventID][]Reaction, error) {
	out := make(map[id.EventID][]Reaction, len(targets))
	if len(targets) == 0 {
		return out, nil
	}
	args := make([]any, len(targets))
	marks := make([]string, len(targets))
	for i, target := range targets {
		args[i] = target
		marks[i] = fmt.Sprintf("$%d", i+1)
	}
	rows, err := queryAll(ctx, s, func(row rowScanner) (Reaction, error) {
		var r Reaction
		err := row.Scan(&r.TargetEventID, &r.Sender, &r.Key)
		return r, err
	}, `SELECT target_event_id, sender, key FROM reaction
		WHERE target_event_id IN (`+strings.Join(marks, ", ")+`) ORDER BY ts, event_id`, args...)
	if err != nil {
		return nil, fmt.Errorf("failed to load reactions: %w", err)
	}
	for _, r := range rows {
		out[r.TargetEventID] = append(out[r.TargetEventID], r)
	}
	return out, nil
}

// aggregateReactions groups the reactions of a message by key, in first-seen order.
func (s *Store) aggregateReactions(ctx context.Context, rc *readCache, roomID id.RoomID, rows []Reaction) ([]MessageReaction, error) {
	out := []MessageReaction{}
	index := map[string]int{}
	for _, r := range rows {
		name := "me"
		if r.Sender != SelfUserID {
			var err error
			if name, _, err = s.sender(ctx, rc, roomID, r.Sender); err != nil {
				return nil, err
			}
		}
		i, ok := index[r.Key]
		if !ok {
			i = len(out)
			index[r.Key] = i
			out = append(out, MessageReaction{Key: r.Key, Senders: []string{}})
		}
		out[i].Count++
		out[i].Senders = append(out[i].Senders, name)
		out[i].FromMe = out[i].FromMe || r.Sender == SelfUserID
	}
	return out, nil
}

// messageFromEvent builds the JSON message for a stored event.
// reactions are the stored reactions of e, as read by reactionsOf.
func (s *Store) messageFromEvent(ctx context.Context, rc *readCache, e *Event, reactions []Reaction) (*Message, error) {
	d, err := describe(e)
	if err != nil {
		return nil, err
	}
	m := &Message{
		EventID:       e.EventID,
		RoomID:        e.RoomID,
		Order:         e.Order,
		Sender:        e.Sender,
		FromMe:        e.Sender == SelfUserID,
		TS:            e.TS,
		Type:          d.Type,
		Body:          d.Body,
		FormattedBody: d.FormattedBody,
		Media:         d.Media,
		Edited:        d.Edited,
		Redacted:      e.Redacted,
		Status:        e.Status,
		Error:         e.Error,
	}
	if m.Status == "" {
		m.Status = "sent"
	}
	name, avatar, err := s.sender(ctx, rc, e.RoomID, e.Sender)
	if err != nil {
		return nil, err
	}
	m.SenderName, m.SenderAvatar = name, string(avatar)
	if e.StateKey != nil {
		m.Type = "state"
		if m.Body, err = s.stateBody(ctx, rc, e); err != nil {
			return nil, err
		}
	}
	if m.Reactions, err = s.aggregateReactions(ctx, rc, e.RoomID, reactions); err != nil {
		return nil, err
	}
	if e.ReplyTo != "" {
		if m.ReplyTo, err = s.replyPreview(ctx, rc, e.ReplyTo); err != nil {
			return nil, err
		}
	}
	return m, nil
}

func (s *Store) replyPreview(ctx context.Context, rc *readCache, target id.EventID) (*MessageReply, error) {
	ev, err := s.GetEvent(ctx, target)
	if err == ErrNotFound {
		return &MessageReply{EventID: target}, nil
	} else if err != nil {
		return nil, err
	}
	d, err := describe(ev)
	if err != nil {
		return nil, err
	}
	name, _, err := s.sender(ctx, rc, ev.RoomID, ev.Sender)
	if err != nil {
		return nil, err
	}
	return &MessageReply{EventID: target, SenderName: name, Body: d.Body}, nil
}

func (s *Store) messagesFor(ctx context.Context, events []*Event) ([]Message, error) {
	targets := make([]id.EventID, len(events))
	for i, e := range events {
		targets[i] = e.EventID
	}
	reactions, err := s.reactionsOf(ctx, targets)
	if err != nil {
		return nil, err
	}
	out := make([]Message, 0, len(events))
	rc := newReadCache()
	for _, e := range events {
		m, err := s.messageFromEvent(ctx, rc, e, reactions[e.EventID])
		if err != nil {
			return nil, err
		}
		out = append(out, *m)
	}
	return out, nil
}

// ListMessages returns up to limit messages of a room, newest last. beforeOrder 0 means the
// latest messages; otherwise only messages with an order below beforeOrder are returned.
func (s *Store) ListMessages(ctx context.Context, roomID id.RoomID, beforeOrder int64, limit int) ([]Message, error) {
	events, err := queryAll(ctx, s, scanEvent, `
		SELECT `+eventColumns+` FROM event
		WHERE room_id = $1 AND ($2 = 0 OR ord < $2)
		ORDER BY ord DESC LIMIT $3`, roomID, beforeOrder, limit)
	if err != nil {
		return nil, fmt.Errorf("failed to list messages: %w", err)
	}
	reverse(events)
	return s.messagesFor(ctx, events)
}

// MessageByID returns one message, or ErrNotFound.
func (s *Store) MessageByID(ctx context.Context, eventID id.EventID) (*Message, error) {
	e, err := s.GetEvent(ctx, eventID)
	if err != nil {
		return nil, err
	}
	msgs, err := s.messagesFor(ctx, []*Event{e})
	if err != nil {
		return nil, err
	}
	return &msgs[0], nil
}

// Search returns up to limit messages whose body contains query, newest first.
func (s *Store) Search(ctx context.Context, query string, limit int) ([]Message, error) {
	events, err := queryAll(ctx, s, scanEvent, `
		SELECT `+eventColumns+` FROM event
		WHERE redacted = false AND type = $1
		  AND COALESCE(json_extract(edit_content_json, '$.body'), json_extract(content_json, '$.body'))
		      LIKE $2 ESCAPE '\'
		ORDER BY ord DESC LIMIT $3`, typeMessage, likePattern(query), limit)
	if err != nil {
		return nil, fmt.Errorf("failed to search messages: %w", err)
	}
	return s.messagesFor(ctx, events)
}

// previewOf describes an event for the chat list, attributed to the sender's display name.
func previewOf(ev *Event, senderName string) (*ChatPreview, error) {
	d, err := describe(ev)
	if err != nil {
		return nil, err
	}
	return &ChatPreview{Sender: firstNonEmpty(senderName, string(ev.Sender)), Body: strings.TrimSpace(previewText(d)), Type: d.Type}, nil
}

func reverse[T any](items []T) {
	for i, j := 0, len(items)-1; i < j; i, j = i+1, j-1 {
		items[i], items[j] = items[j], items[i]
	}
}
