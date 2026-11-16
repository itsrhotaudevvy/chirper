package localmx

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"time"

	"maunium.net/go/mautrix"
	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/event"
	"maunium.net/go/mautrix/id"

	"chirper.dev/core/store"
)

// intent is the MatrixAPI of one Matrix identity: the bot, a ghost or the local user.
type intent struct {
	c            *Connector
	mxid         id.UserID
	doublePuppet bool
}

var _ bridgev2.MatrixAPI = (*intent)(nil)

func (c *Connector) newIntent(mxid id.UserID, doublePuppet bool) *intent {
	return &intent{c: c, mxid: mxid, doublePuppet: doublePuppet}
}

// msOrNow converts a bridge timestamp to Unix milliseconds, using the current time when unset.
func msOrNow(ts time.Time) int64 {
	if ts.IsZero() {
		return nowMS()
	}
	return ts.UnixMilli()
}

// GetMXID implements bridgev2.MatrixAPI.
func (i *intent) GetMXID() id.UserID { return i.mxid }

// IsDoublePuppet implements bridgev2.MatrixAPI.
func (i *intent) IsDoublePuppet() bool { return i.doublePuppet }

// SendMessage implements bridgev2.MatrixAPI.
func (i *intent) SendMessage(ctx context.Context, roomID id.RoomID, eventType event.Type, content *event.Content,
	extra *bridgev2.MatrixSendExtra,
) (*mautrix.RespSendEvent, error) {
	if extra == nil {
		extra = &bridgev2.MatrixSendExtra{}
	}
	eventID, err := i.c.dispatch(ctx, roomID, i.mxid, eventType, content, i.c.eventIDFor(roomID, extra),
		msOrNow(extra.Timestamp), extra.TransactionID, arrivedLive)
	if err != nil {
		return nil, err
	}
	return &mautrix.RespSendEvent{EventID: eventID}, nil
}

// SendState implements bridgev2.MatrixAPI.
func (i *intent) SendState(ctx context.Context, roomID id.RoomID, eventType event.Type, stateKey string,
	content *event.Content, ts time.Time,
) (*mautrix.RespSendEvent, error) {
	eventID, err := i.c.sendState(ctx, roomID, i.mxid, eventType, stateKey, content, msOrNow(ts))
	if err != nil {
		return nil, err
	}
	return &mautrix.RespSendEvent{EventID: eventID}, nil
}

// MarkRead implements bridgev2.MatrixAPI. Only the local user's read position is tracked; read
// receipts of ghosts are ignored.
func (i *intent) MarkRead(ctx context.Context, roomID id.RoomID, eventID id.EventID, _ time.Time) error {
	if !i.doublePuppet {
		return nil
	}
	err := i.c.st.MarkReadUpTo(ctx, roomID, eventID)
	if errors.Is(err, store.ErrNotFound) {
		return nil
	} else if err != nil {
		return err
	}
	i.c.sink.RoomChanged(roomID)
	return nil
}

// MarkUnread implements bridgev2.MatrixAPI.
func (i *intent) MarkUnread(ctx context.Context, roomID id.RoomID, unread bool) error {
	if err := i.c.st.MarkUnread(ctx, roomID, unread); err != nil {
		return err
	}
	i.c.sink.RoomChanged(roomID)
	return nil
}

// MarkTyping implements bridgev2.MatrixAPI. Only text typing is shown.
func (i *intent) MarkTyping(_ context.Context, roomID id.RoomID, typingType bridgev2.TypingType, timeout time.Duration) error {
	if typingType != bridgev2.TypingTypeText {
		return nil
	}
	i.c.setTyping(roomID, i.mxid, timeout)
	return nil
}

// CreateRoom implements bridgev2.MatrixAPI. The room is stored with the state and members the
// bridge asked for.
func (i *intent) CreateRoom(ctx context.Context, req *mautrix.ReqCreateRoom) (id.RoomID, error) {
	roomID := req.BeeperLocalRoomID
	if roomID == "" {
		roomID = i.c.newRoomID()
	}
	err := i.c.st.UpsertRoom(ctx, &store.Room{
		RoomID:    roomID,
		BridgeID:  string(i.c.bridgeID),
		IsDM:      req.IsDirect,
		IsSpace:   fmt.Sprint(req.CreationContent["type"]) == string(event.RoomTypeSpace),
		CreatedTS: nowMS(),
	})
	if err != nil {
		return "", err
	}
	if err := i.c.applyRoomCreate(ctx, roomID, req); err != nil {
		return "", err
	}
	i.c.sink.RoomChanged(roomID)
	return roomID, nil
}

// applyRoomCreate stores the name, topic, avatar and members of a new room.
func (c *Connector) applyRoomCreate(ctx context.Context, roomID id.RoomID, req *mautrix.ReqCreateRoom) error {
	fields := store.RoomFields{}
	if req.Name != "" {
		fields.Name = &req.Name
	}
	if req.Topic != "" {
		fields.Topic = &req.Topic
	}
	for _, evt := range req.InitialState {
		if err := c.applyInitialState(ctx, roomID, evt, &fields); err != nil {
			return err
		}
	}
	if err := c.st.SetRoomFields(ctx, roomID, fields); err != nil {
		return err
	}
	for _, userID := range req.Invite {
		if err := c.st.SetMembership(ctx, roomID, userID, event.MembershipInvite); err != nil {
			return err
		}
	}
	for _, userID := range req.BeeperInitialMembers {
		if err := c.st.SetMembership(ctx, roomID, userID, event.MembershipJoin); err != nil {
			return err
		}
	}
	return c.st.SetMembership(ctx, roomID, c.botUserID(), event.MembershipJoin)
}

// applyInitialState folds one initial state event of a room creation into fields or member rows.
func (c *Connector) applyInitialState(ctx context.Context, roomID id.RoomID, evt *event.Event, fields *store.RoomFields) error {
	switch evt.Type {
	case event.StateRoomName:
		nc, err := parseContent[event.RoomNameEventContent](&evt.Content)
		if err != nil {
			return err
		}
		fields.Name = &nc.Name
	case event.StateTopic:
		tc, err := parseContent[event.TopicEventContent](&evt.Content)
		if err != nil {
			return err
		}
		fields.Topic = &tc.Topic
	case event.StateRoomAvatar:
		ac, err := parseContent[event.RoomAvatarEventContent](&evt.Content)
		if err != nil {
			return err
		}
		url := string(ac.URL)
		fields.AvatarURL = &url
	case event.StateMember:
		if evt.StateKey == nil {
			return nil
		}
		_, err := c.applyMember(ctx, roomID, *evt.StateKey, &evt.Content)
		return err
	}
	return nil
}

// DeleteRoom implements bridgev2.MatrixAPI. The room row is kept, marked deleted. Its messages,
// reactions, members and media are erased.
func (i *intent) DeleteRoom(ctx context.Context, roomID id.RoomID, _ bool) error {
	if err := i.c.st.DeleteRoom(ctx, roomID); err != nil {
		return err
	}
	i.c.sink.RoomChanged(roomID)
	return nil
}

// EnsureJoined implements bridgev2.MatrixAPI.
func (i *intent) EnsureJoined(ctx context.Context, roomID id.RoomID, _ ...bridgev2.EnsureJoinedParams) error {
	return i.setMembership(ctx, roomID, i.mxid, event.MembershipJoin)
}

// EnsureInvited implements bridgev2.MatrixAPI.
func (i *intent) EnsureInvited(ctx context.Context, roomID id.RoomID, userID id.UserID) error {
	return i.setMembership(ctx, roomID, userID, event.MembershipInvite)
}

func (i *intent) setMembership(ctx context.Context, roomID id.RoomID, userID id.UserID, membership event.Membership) error {
	if err := i.c.st.SetMembership(ctx, roomID, userID, membership); err != nil {
		return err
	}
	i.c.sink.RoomChanged(roomID)
	return nil
}

// TagRoom implements bridgev2.MatrixAPI. Favourite pins the chat and low priority archives it;
// other tags have no local meaning.
func (i *intent) TagRoom(ctx context.Context, roomID id.RoomID, tag event.RoomTag, isTagged bool) error {
	var fields store.RoomFields
	switch tag {
	case event.RoomTagFavourite:
		fields.Pinned = &isTagged
	case event.RoomTagLowPriority:
		fields.Archived = &isTagged
	default:
		return nil
	}
	if err := i.c.st.SetRoomFields(ctx, roomID, fields); err != nil {
		return err
	}
	i.c.sink.RoomChanged(roomID)
	return nil
}

// MuteRoom implements bridgev2.MatrixAPI. The mute is stored as Unix milliseconds: 0 when not
// muted and -1 when muted forever.
func (i *intent) MuteRoom(ctx context.Context, roomID id.RoomID, until time.Time) error {
	var mutedUntil int64
	switch {
	case until.Equal(event.MutedForever):
		mutedUntil = -1
	case until.Before(time.Now()):
		mutedUntil = 0
	default:
		mutedUntil = until.UnixMilli()
	}
	if err := i.c.st.SetRoomFields(ctx, roomID, store.RoomFields{MutedUntil: &mutedUntil}); err != nil {
		return err
	}
	i.c.sink.RoomChanged(roomID)
	return nil
}

// GetEvent implements bridgev2.MatrixAPI. It returns the stored content, before any edit.
func (i *intent) GetEvent(ctx context.Context, roomID id.RoomID, eventID id.EventID) (*event.Event, error) {
	e, err := i.c.st.GetEvent(ctx, eventID)
	if err != nil {
		return nil, err
	}
	if e.RoomID != roomID {
		return nil, store.ErrNotFound
	}
	evtType := event.Type{Type: e.Type, Class: event.MessageEventType}
	if e.StateKey != nil {
		evtType.Class = event.StateEventType
	}
	var content event.Content
	if err := json.Unmarshal([]byte(e.ContentJSON), &content); err != nil {
		return nil, fmt.Errorf("failed to parse stored content: %w", err)
	}
	// Unknown event types keep their raw content, so the parse error is not fatal here.
	_ = content.ParseRaw(evtType)
	return &event.Event{
		ID:        e.EventID,
		RoomID:    e.RoomID,
		Sender:    e.Sender,
		Type:      evtType,
		StateKey:  e.StateKey,
		Timestamp: e.TS,
		Content:   content,
	}, nil
}
