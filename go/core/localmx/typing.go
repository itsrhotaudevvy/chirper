package localmx

import (
	"fmt"
	"time"

	"maunium.net/go/mautrix/id"
)

// setTyping records that userID is typing in roomID for timeout (0 stops typing), expires stale
// entries, and notifies the sink with the users still typing.
func (c *Connector) setTyping(roomID id.RoomID, userID id.UserID, timeout time.Duration) {
	c.typingMu.Lock()
	users := c.typingMap(roomID)
	if timeout > 0 {
		users[userID] = time.Now().Add(timeout)
		time.AfterFunc(timeout, func() { c.expireTypingSafe(roomID) })
	} else {
		delete(users, userID)
	}
	active := activeTyping(users, time.Now())
	c.typingMu.Unlock()
	c.sink.Typing(roomID, active)
}

// expireTypingSafe is expireTyping run from a timer goroutine: a panic is reported to the sink, when
// it accepts reports, instead of ending the host process.
func (c *Connector) expireTypingSafe(roomID id.RoomID) {
	defer func() {
		if v := recover(); v != nil {
			if r, ok := c.sink.(errorReporter); ok {
				r.ReportError(fmt.Errorf("internal error: %v", v))
			}
		}
	}()
	c.expireTyping(roomID)
}

// expireTyping drops expired typing entries of a room and notifies the sink.
func (c *Connector) expireTyping(roomID id.RoomID) {
	c.typingMu.Lock()
	users := c.typingMap(roomID)
	active := activeTyping(users, time.Now())
	c.typingMu.Unlock()
	c.sink.Typing(roomID, active)
}

// typingMap returns the typing entries of a room. The caller must hold typingMu.
func (c *Connector) typingMap(roomID id.RoomID) map[id.UserID]time.Time {
	users, ok := c.typing[roomID]
	if !ok {
		users = make(map[id.UserID]time.Time)
		c.typing[roomID] = users
	}
	return users
}

// activeTyping deletes expired entries and returns the remaining users.
func activeTyping(users map[id.UserID]time.Time, now time.Time) []id.UserID {
	out := make([]id.UserID, 0, len(users))
	for user, expires := range users {
		if now.After(expires) {
			delete(users, user)
			continue
		}
		out = append(out, user)
	}
	return out
}
