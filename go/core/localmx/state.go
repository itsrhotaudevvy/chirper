package localmx

import (
	"context"
	"encoding/json"
	"fmt"

	"maunium.net/go/mautrix/event"
	"maunium.net/go/mautrix/id"

	"chirper.dev/core/store"
)

// sendState applies a state event sent through an intent. Room name, topic, avatar and member
// changes are stored; other state types are accepted and ignored, because they have no local
// representation. Visible changes are also recorded as timeline events.
func (c *Connector) sendState(ctx context.Context, roomID id.RoomID, sender id.UserID, eventType event.Type,
	stateKey string, content *event.Content, ts int64,
) (id.EventID, error) {
	eventID := newLocalEventID()
	visible := false
	switch eventType {
	case event.StateRoomName:
		nc, err := parseContent[event.RoomNameEventContent](content)
		if err != nil {
			return "", err
		}
		if err := c.st.SetRoomFields(ctx, roomID, store.RoomFields{Name: &nc.Name}); err != nil {
			return "", err
		}
		visible = true
	case event.StateTopic:
		tc, err := parseContent[event.TopicEventContent](content)
		if err != nil {
			return "", err
		}
		if err := c.st.SetRoomFields(ctx, roomID, store.RoomFields{Topic: &tc.Topic}); err != nil {
			return "", err
		}
		visible = true
	case event.StateRoomAvatar:
		av, err := parseContent[event.RoomAvatarEventContent](content)
		if err != nil {
			return "", err
		}
		url := string(av.URL)
		if err := c.st.SetRoomFields(ctx, roomID, store.RoomFields{AvatarURL: &url}); err != nil {
			return "", err
		}
		visible = true
	case event.StateMember:
		var err error
		if visible, err = c.applyMember(ctx, roomID, stateKey, content); err != nil {
			return "", err
		}
	}
	if !visible {
		return eventID, nil
	}
	raw, err := json.Marshal(content)
	if err != nil {
		return "", fmt.Errorf("failed to marshal state content: %w", err)
	}
	key := stateKey
	_, err = c.st.InsertEvent(ctx, &store.Event{
		EventID:     eventID,
		RoomID:      roomID,
		Sender:      sender,
		Type:        eventType.Type,
		StateKey:    &key,
		ContentJSON: string(raw),
		TS:          ts,
	})
	if err != nil {
		return "", err
	}
	c.sink.RoomChanged(roomID)
	c.sink.EventChanged(roomID, eventID, false)
	return eventID, nil
}

// applyMember upserts the member row and reports whether the change is worth a timeline entry:
// joins and leaves in group rooms are, invites and DM changes are not.
func (c *Connector) applyMember(ctx context.Context, roomID id.RoomID, stateKey string, content *event.Content) (bool, error) {
	mc, err := parseContent[event.MemberEventContent](content)
	if err != nil {
		return false, err
	}
	userID := id.UserID(stateKey)
	err = c.st.UpsertMember(ctx, &store.Member{
		RoomID:      roomID,
		UserID:      userID,
		Displayname: mc.Displayname,
		AvatarURL:   mc.AvatarURL,
		Membership:  mc.Membership,
	})
	if err != nil {
		return false, err
	}
	if mc.Membership != event.MembershipJoin && mc.Membership != event.MembershipLeave {
		return false, nil
	}
	room, err := c.st.GetRoom(ctx, roomID)
	if err != nil {
		return false, err
	}
	return !room.IsDM, nil
}
