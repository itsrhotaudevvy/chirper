package engine

import (
	"context"
	"errors"

	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/networkid"
	"maunium.net/go/mautrix/bridgev2/status"
	"maunium.net/go/mautrix/id"

	"chirper.dev/core/localmx"
	"chirper.dev/core/store"
)

// bridgeSink receives the change notifications of one bridge's localmx connector. Every method
// only queues work, so the bridge is never blocked on the listener or on the store.
type bridgeSink struct {
	e        *Engine
	bridgeID networkid.BridgeID
}

var _ localmx.EventSink = (*bridgeSink)(nil)

// RoomChanged implements localmx.EventSink.
func (s *bridgeSink) RoomChanged(roomID id.RoomID) {
	s.e.queueChat(roomID)
}

// EventChanged implements localmx.EventSink. An edit or reaction can change the chat preview too.
func (s *bridgeSink) EventChanged(roomID id.RoomID, eventID id.EventID, live bool) {
	s.e.queueMessage(eventID, live)
	s.e.queueChat(roomID)
}

// EventRemoved implements localmx.EventSink.
func (s *bridgeSink) EventRemoved(roomID id.RoomID, eventID id.EventID) {
	s.e.disp.emit("message_removed", mustJSON(map[string]string{"roomId": string(roomID), "eventId": string(eventID)}))
}

// Typing implements localmx.EventSink.
func (s *bridgeSink) Typing(roomID id.RoomID, userIDs []id.UserID) {
	s.e.setTyping(roomID, userIDs)
	s.e.queueTyping(roomID)
	s.e.queueChat(roomID)
}

// BridgeState implements localmx.EventSink. States without a login (bridge-wide ones) are ignored.
func (s *bridgeSink) BridgeState(_ networkid.BridgeID, state status.BridgeState) {
	if state.RemoteID == "" {
		return
	}
	key := accountKey{network: string(s.bridgeID), loginID: string(state.RemoteID)}
	s.e.mu.Lock()
	s.e.states[key] = state
	s.e.mu.Unlock()
	s.e.queueAccount(s.bridgeID, string(state.RemoteID))
}

// MessageStatus implements localmx.EventSink. A failed optimistic write is undone here.
func (s *bridgeSink) MessageStatus(_ id.RoomID, eventID id.EventID, st *bridgev2.MessageStatus) {
	s.e.settle(eventID, st.Status)
	s.e.queueMessage(eventID, false)
}

// ReportError implements localmx.EventSink.
func (s *bridgeSink) ReportError(err error) {
	s.e.reportError(err)
}

// queueChat schedules a chat event for roomID.
func (e *Engine) queueChat(roomID id.RoomID) {
	e.disp.queue("chat:"+string(roomID), func() (string, string, bool) {
		chat, err := e.loadChat(e.ctx, roomID)
		if errors.Is(err, store.ErrNotFound) {
			return "chat_removed", mustJSON(map[string]string{"roomId": string(roomID)}), true
		} else if err != nil {
			e.log.Error().Err(err).Msg("Failed to load chat")
			return "", "", false
		}
		return "chat", mustJSON(chat), true
	})
}

// queueMessage schedules a message event for eventID. live marks the message as arriving in real
// time. Changes queued before the event is delivered are coalesced into it, so the flag is kept
// until the event is delivered, whichever change queued it last.
func (e *Engine) queueMessage(eventID id.EventID, live bool) {
	if live {
		e.mu.Lock()
		e.liveEvents[eventID] = true
		e.mu.Unlock()
	}
	e.disp.queue("message:"+string(eventID), func() (string, string, bool) {
		isLive := e.takeLive(eventID)
		msg, err := e.st.MessageByID(e.ctx, eventID)
		if err != nil {
			return "", "", false
		}
		msg.Live = isLive
		return "message", mustJSON(msg), true
	})
}

// takeLive reports whether eventID is pending as live and clears the mark.
func (e *Engine) takeLive(eventID id.EventID) bool {
	e.mu.Lock()
	defer e.mu.Unlock()
	live := e.liveEvents[eventID]
	delete(e.liveEvents, eventID)
	return live
}

// queueTyping schedules the typing event of roomID.
func (e *Engine) queueTyping(roomID id.RoomID) {
	e.disp.queue("typing:"+string(roomID), func() (string, string, bool) {
		names := e.typingNames(e.ctx, roomID)
		return "typing", mustJSON(map[string]any{"roomId": roomID, "names": names}), true
	})
}

// queueAccount schedules the account event of a login.
func (e *Engine) queueAccount(bridgeID networkid.BridgeID, loginID string) {
	e.disp.queue("account:"+string(bridgeID)+":"+loginID, func() (string, string, bool) {
		acc := e.accountFor(string(bridgeID), loginID)
		return "account", mustJSON(acc), true
	})
}

// accountFor builds the account of a login, from the cached login when it still exists.
func (e *Engine) accountFor(network, loginID string) Account {
	state := e.lastState(network, loginID)
	e.mu.Lock()
	inst := e.instances[networkid.BridgeID(network)]
	e.mu.Unlock()
	var login *bridgev2.UserLogin
	if inst != nil {
		login = inst.br.GetCachedUserLoginByID(networkid.UserLoginID(loginID))
	}
	return e.accountOf(network, login, loginID, state)
}

// accountOf builds an Account from a login (which may be nil) and its last bridge state.
func (e *Engine) accountOf(network string, login *bridgev2.UserLogin, loginID string, state status.BridgeState) Account {
	acc := Account{
		Network: network,
		LoginID: loginID,
		State:   accountState(state.StateEvent),
		Error:   string(state.Error),
		Message: state.Message,
	}
	if login != nil {
		acc.Name = firstNonEmpty(login.RemoteName, login.RemoteProfile.Name, login.RemoteProfile.Phone, login.RemoteProfile.Username)
		acc.AvatarURL = string(login.RemoteProfile.Avatar)
	}
	return acc
}

// setTyping records who is typing in roomID now.
func (e *Engine) setTyping(roomID id.RoomID, userIDs []id.UserID) {
	e.mu.Lock()
	defer e.mu.Unlock()
	if len(userIDs) == 0 {
		delete(e.typing, roomID)
		return
	}
	e.typing[roomID] = append([]id.UserID(nil), userIDs...)
}

// typingNames returns the display names of the users typing in roomID.
func (e *Engine) typingNames(ctx context.Context, roomID id.RoomID) []string {
	e.mu.Lock()
	users := e.typing[roomID]
	e.mu.Unlock()
	names := []string{}
	for _, u := range users {
		name, err := e.st.DisplayName(ctx, roomID, u)
		if err != nil {
			continue
		}
		names = append(names, name)
	}
	return names
}
