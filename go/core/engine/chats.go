package engine

import (
	"context"
	"errors"
	"fmt"

	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/id"

	"chirper.dev/core/store"
)

const (
	defaultPageSize = 50
	maxPageSize     = 200
)

// clampLimit returns limit within [1, maxPageSize], or defaultPageSize when limit is not positive.
func clampLimit(limit int) int {
	switch {
	case limit <= 0:
		return defaultPageSize
	case limit > maxPageSize:
		return maxPageSize
	}
	return limit
}

// ListChats returns the chats matching filter, with the users typing in each.
func (e *Engine) ListChats(filter store.ChatFilter) ([]store.Chat, error) {
	chats, err := e.st.ListChats(e.ctx, filter)
	if err != nil {
		return nil, err
	}
	for i := range chats {
		if chats[i].LoginID == "" || (chats[i].ParentID == "" && !chats[i].IsDM) {
			e.fillRoomIdentity(&chats[i])
		}
		chats[i].Typing = e.typingNames(e.ctx, chats[i].RoomID)
	}
	return chats, nil
}

// fillRoomIdentity resolves the login and parent server of a chat that does not have them yet.
func (e *Engine) fillRoomIdentity(chat *store.Chat) {
	room, err := e.st.GetRoom(e.ctx, chat.RoomID)
	if err != nil {
		return
	}
	if login, err := e.attachRoomLogin(e.ctx, room); err == nil && login != nil {
		chat.LoginID = string(login.ID)
	}
	if room.ParentID != "" {
		chat.ParentID = room.ParentID
		chat.IsChannel = true
		chat.IsGroup = false
		if chat.ServerName == "" {
			if parentRoom, err := e.st.GetRoom(e.ctx, id.RoomID(room.ParentID)); err == nil && parentRoom.Name != "" {
				chat.ServerName = parentRoom.Name
			}
		}
	}
}

// fillLoginID resolves the login of a chat that does not have one yet. Chats whose login is still
// unknown are left as they are.
func (e *Engine) fillLoginID(chat *store.Chat) {
	e.fillRoomIdentity(chat)
}

// GetChat returns one chat. Deleted chats are reported as store.ErrNotFound.
func (e *Engine) GetChat(roomID id.RoomID) (*store.Chat, error) {
	return e.loadChat(e.ctx, roomID)
}

// loadChat builds the chat of roomID for events and GetChat. It also fills in the login of a room
// that was created before its login was known.
func (e *Engine) loadChat(ctx context.Context, roomID id.RoomID) (*store.Chat, error) {
	room, err := e.st.GetRoom(ctx, roomID)
	if err != nil {
		return nil, err
	}
	if room.Deleted {
		return nil, store.ErrNotFound
	}
	if room.LoginID == "" || (room.ParentID == "" && !room.IsDM) {
		_, _ = e.attachRoomLogin(ctx, room)
	}
	chat, err := e.st.GetChat(ctx, roomID)
	if err != nil {
		return nil, err
	}
	chat.Typing = e.typingNames(ctx, roomID)
	return chat, nil
}

// attachRoomLogin stores the login and portal of a room whose login is not known yet. It returns
// the login, or an error when no logged-in login is in the room.
func (e *Engine) attachRoomLogin(ctx context.Context, room *store.Room) (*bridgev2.UserLogin, error) {
	inst, err := e.runningInstance(room.BridgeID)
	if err != nil {
		return nil, err
	}
	portal, err := inst.br.GetPortalByMXID(ctx, room.RoomID)
	if err != nil {
		return nil, fmt.Errorf("failed to load chat portal: %w", err)
	}
	if portal == nil {
		return nil, errors.New("chat has no portal")
	}
	user, err := inst.br.GetUserByMXID(ctx, store.SelfUserID)
	if err != nil {
		return nil, fmt.Errorf("failed to load user: %w", err)
	}
	login, _, err := portal.FindPreferredLogin(ctx, user, true)
	if err != nil {
		return nil, fmt.Errorf("no logged-in account in chat: %w", err)
	}
	loginID, portalID := string(login.ID), string(portal.PortalKey.ID)
	fields := store.RoomFields{LoginID: &loginID, PortalID: &portalID}

	if room.ParentID == "" {
		var parentID string
		if portal.Parent != nil && portal.Parent.MXID != "" {
			parentID = string(portal.Parent.MXID)
		} else if portal.ParentKey.ID != "" {
			parentKey := portal.ParentKey
			if parentKey.Receiver == "" {
				parentKey.Receiver = portal.PortalKey.Receiver
			}
			if parentPortal, _ := inst.br.GetPortalByKey(ctx, parentKey); parentPortal != nil && parentPortal.MXID != "" {
				parentID = string(parentPortal.MXID)
			} else if spaceRoom, err := e.st.FindSpaceRoom(ctx, room.BridgeID, string(portal.ParentKey.ID)); err == nil && spaceRoom != nil {
				parentID = string(spaceRoom.RoomID)
			} else {
				parentID = string(portal.ParentKey.ID)
			}
		}
		if parentID != "" {
			fields.ParentID = &parentID
			room.ParentID = parentID
		}
	}

	if err := e.st.SetRoomFields(ctx, room.RoomID, fields); err != nil {
		return nil, err
	}
	room.LoginID, room.PortalID = loginID, portalID
	return login, nil
}

// roomRef is a chat resolved for an outgoing operation.
type roomRef struct {
	room  *store.Room
	inst  *bridgeInstance
	login *bridgev2.UserLogin
}

// resolveRoom finds the bridge and the logged-in account that send in roomID.
func (e *Engine) resolveRoom(ctx context.Context, roomID id.RoomID) (roomRef, error) {
	room, err := e.st.GetRoom(ctx, roomID)
	if errors.Is(err, store.ErrNotFound) || (err == nil && room.Deleted) {
		return roomRef{}, fmt.Errorf("unknown chat %s", roomID)
	} else if err != nil {
		return roomRef{}, err
	}
	inst, err := e.runningInstance(room.BridgeID)
	if err != nil {
		return roomRef{}, err
	}
	login, err := e.attachRoomLogin(ctx, room)
	if err != nil {
		return roomRef{}, fmt.Errorf("cannot send in this chat: %w", err)
	}
	return roomRef{room: room, inst: inst, login: login}, nil
}

// ListMessages returns up to limit messages of roomID, newest last. beforeOrder 0 means the latest.
func (e *Engine) ListMessages(roomID id.RoomID, beforeOrder int64, limit int) ([]store.Message, error) {
	return e.st.ListMessages(e.ctx, roomID, beforeOrder, clampLimit(limit))
}

// Search returns up to limit messages whose text contains query, newest first.
func (e *Engine) Search(query string, limit int) ([]store.Message, error) {
	return e.st.Search(e.ctx, query, clampLimit(limit))
}
