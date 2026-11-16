// Package openchirp is the gomobile facade of the Chirper core. It exposes only types that
// gomobile can bind: strings, numbers, booleans, errors and one callback interface. Structured
// values cross the boundary as UTF-8 JSON with camelCase keys (see docs/ARCHITECTURE.md).
package facade

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"sync"

	"chirper.dev/core/engine"
	"chirper.dev/core/store"
)

// Input limits. Oversized input is refused with an error before it reaches the engine.
const (
	maxTextBytes     = 64 << 10 // message text, captions and edits
	maxKeyBytes      = 64       // reaction keys
	maxQueryBytes    = 256      // search and chat-name queries
	maxIDBytes       = 256      // room, event, network, login, process and transaction IDs, mxc URIs
	maxPathBytes     = 4096     // local file paths
	maxFileNameBytes = 1024     // file names sent with media
	maxCookieBytes   = 64 << 10 // the whole cookie payload of one login step
	maxLimit         = 500      // page sizes
)

// EventListener receives core events. kind and payload are described in docs/ARCHITECTURE.md.
// Calls come from one goroutine, so the implementation must not block for long.
type EventListener interface {
	OnEvent(kind string, payload string)
}

// serialListener serialises calls to the app's listener. Engine events come from the dispatcher
// goroutine and facade errors from the calling goroutine, so both pass through this lock.
type serialListener struct {
	mu sync.Mutex
	l  EventListener
}

// OnEvent implements engine.Listener.
func (s *serialListener) OnEvent(kind, payload string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.l.OnEvent(kind, payload)
}

// Core is the Chirper core. Create it with NewCore, call Start, and Stop when the app goes away.
type Core struct {
	eng      *engine.Engine
	listener *serialListener
}

// NewCore opens the core under dataDir without database encryption. Nothing is started until Start.
func NewCore(dataDir string, listener EventListener) (*Core, error) {
	return NewCoreWithKey(dataDir, nil, listener)
}

// NewCoreWithKey opens the core under dataDir with the databases encrypted under key, when the
// build has SQLCipher. key must be nil (no encryption) or exactly engine.DBKeyBytes long. The core
// keeps its own copy and zeroes it in Stop, so the caller may clear its copy after the call.
func NewCoreWithKey(dataDir string, key []byte, listener EventListener) (c *Core, err error) {
	defer recoverErr(&err)
	if listener == nil {
		return nil, errors.New("nil event listener")
	}
	if len(key) == 0 {
		key = nil
	} else if len(key) != engine.DBKeyBytes {
		return nil, fmt.Errorf("database key must be %d bytes", engine.DBKeyBytes)
	}
	sl := &serialListener{l: listener}
	eng, err := engine.NewWithOptions(engine.Options{DataDir: dataDir, DBKey: key}, sl)
	if err != nil {
		return nil, err
	}
	return &Core{eng: eng, listener: sl}, nil
}

// Start starts the bridges that have logins. It returns once they are starting.
func (c *Core) Start() (err error) {
	defer c.recoverTo(&err)
	return c.eng.Start(context.Background())
}

// Stop stops all networks and closes the databases.
func (c *Core) Stop() {
	defer c.recoverTo(nil)
	c.eng.Stop()
}

// Networks returns the JSON []Network that can be used now.
func (c *Core) Networks() (out string) {
	defer c.recoverTo(nil)
	return mustJSON(c.eng.Networks())
}

// Accounts returns the JSON []Account of every login.
func (c *Core) Accounts() (out string) {
	defer c.recoverTo(nil)
	accounts, err := c.eng.Accounts()
	if err != nil {
		c.report(err)
		return "[]"
	}
	return mustJSON(accounts)
}

// StartLogin begins a login flow and returns the JSON LoginState of its first step.
func (c *Core) StartLogin(network, flowID string) (out string, err error) {
	defer c.recoverTo(&err)
	if err := checkAll(maxLen("network", network, maxIDBytes), maxLen("flowID", flowID, maxIDBytes)); err != nil {
		return "", err
	}
	state, err := c.eng.StartLogin(network, flowID)
	if err != nil {
		return "", err
	}
	return mustJSON(state), nil
}

// SubmitLoginInput submits {"fieldId":"value"} for a user input step.
func (c *Core) SubmitLoginInput(processID, valuesJSON string) (out string, err error) {
	defer c.recoverTo(&err)
	if err := checkAll(maxLen("processID", processID, maxIDBytes), maxLen("values", valuesJSON, maxTextBytes)); err != nil {
		return "", err
	}
	values, err := parseStringMap(valuesJSON, "login input")
	if err != nil {
		return "", err
	}
	state, err := c.eng.SubmitLoginInput(processID, values)
	if err != nil {
		return "", err
	}
	return mustJSON(state), nil
}

// SubmitLoginCookies submits {"fieldId":"value"} for a cookies step. The payload is capped at 64 KiB,
// and entries with control characters are dropped.
func (c *Core) SubmitLoginCookies(processID, cookiesJSON string) (out string, err error) {
	defer c.recoverTo(&err)
	if err := maxLen("processID", processID, maxIDBytes); err != nil {
		return "", err
	}
	cookies, err := parseCookies(cookiesJSON)
	if err != nil {
		return "", err
	}
	state, err := c.eng.SubmitLoginCookies(processID, cookies)
	if err != nil {
		return "", err
	}
	return mustJSON(state), nil
}

// WaitLoginStep blocks until a display-and-wait step (such as a QR scan) completes.
func (c *Core) WaitLoginStep(processID string) (out string, err error) {
	defer c.recoverTo(&err)
	if err := maxLen("processID", processID, maxIDBytes); err != nil {
		return "", err
	}
	state, err := c.eng.WaitLoginStep(processID)
	if err != nil {
		return "", err
	}
	return mustJSON(state), nil
}

// CancelLogin abandons a login process.
func (c *Core) CancelLogin(processID string) {
	defer c.recoverTo(nil)
	if err := maxLen("processID", processID, maxIDBytes); err != nil {
		c.report(err)
		return
	}
	c.eng.CancelLogin(processID)
}

// Logout logs a login out and removes it.
func (c *Core) Logout(network, loginID string) (err error) {
	defer c.recoverTo(&err)
	if err := checkAll(maxLen("network", network, maxIDBytes), maxLen("loginID", loginID, maxIDBytes)); err != nil {
		return err
	}
	return c.eng.Logout(network, loginID)
}

// ListChats returns the JSON []Chat matching filterJSON, such as {"network":"","archived":false}.
// An empty filter lists every chat.
func (c *Core) ListChats(filterJSON string) (out string) {
	defer c.recoverTo(nil)
	var filter store.ChatFilter
	if filterJSON != "" {
		if err := json.Unmarshal([]byte(filterJSON), &filter); err != nil {
			c.report(errors.New("invalid chat filter"))
			return "[]"
		}
	}
	if err := checkAll(maxLen("network", filter.Network, maxIDBytes), maxLen("query", filter.Query, maxQueryBytes)); err != nil {
		c.report(err)
		return "[]"
	}
	chats, err := c.eng.ListChats(filter)
	if err != nil {
		c.report(err)
		return "[]"
	}
	return mustJSON(chats)
}

// GetChat returns the JSON Chat of roomID, or "" when there is no such chat.
func (c *Core) GetChat(roomID string) (out string) {
	defer c.recoverTo(nil)
	if err := maxLen("roomID", roomID, maxIDBytes); err != nil {
		c.report(err)
		return ""
	}
	chat, err := c.eng.GetChat(roomIDOf(roomID))
	if errors.Is(err, store.ErrNotFound) {
		return ""
	} else if err != nil {
		c.report(err)
		return ""
	}
	return mustJSON(chat)
}

// ListMessages returns the JSON []Message of roomID, newest last. beforeOrder 0 means the latest.
func (c *Core) ListMessages(roomID string, beforeOrder int64, limit int) (out string) {
	defer c.recoverTo(nil)
	if err := maxLen("roomID", roomID, maxIDBytes); err != nil {
		c.report(err)
		return "[]"
	}
	msgs, err := c.eng.ListMessages(roomIDOf(roomID), beforeOrder, clampLimit(limit))
	if err != nil {
		c.report(err)
		return "[]"
	}
	return mustJSON(msgs)
}

// LoadOlder asks the network for older history of roomID. hasMore is false at the beginning of the history.
func (c *Core) LoadOlder(roomID string) (hasMore bool, err error) {
	defer c.recoverTo(&err)
	if err := maxLen("roomID", roomID, maxIDBytes); err != nil {
		return false, err
	}
	return c.eng.LoadOlder(roomIDOf(roomID))
}

// SyncHistory pulls past history for every chat of network (all networks when empty).
func (c *Core) SyncHistory(network string) (err error) {
	defer c.recoverTo(&err)
	if err := maxLen("network", network, maxIDBytes); err != nil {
		return err
	}
	return c.eng.SyncHistory(network)
}

// MarkUnread marks roomID unread, or read when unread is false.
func (c *Core) MarkUnread(roomID string, unread bool) (err error) {
	defer c.recoverTo(&err)
	if err := maxLen("roomID", roomID, maxIDBytes); err != nil {
		return err
	}
	return c.eng.MarkUnread(roomIDOf(roomID), unread)
}

// SendText sends a text message and returns its local event ID.
func (c *Core) SendText(roomID, text, replyToEventID, txnID string) (out string, err error) {
	defer c.recoverTo(&err)
	if err := checkAll(
		maxLen("roomID", roomID, maxIDBytes), maxLen("text", text, maxTextBytes),
		maxLen("replyTo", replyToEventID, maxIDBytes), maxLen("txnID", txnID, maxIDBytes),
	); err != nil {
		return "", err
	}
	eventID, err := c.eng.SendText(roomIDOf(roomID), text, eventIDOf(replyToEventID), txnID)
	return string(eventID), err
}

// SendMedia sends a local file. msgKind is one of image, video, audio, file, sticker, voice.
func (c *Core) SendMedia(roomID, filePath, mimeType, fileName, caption, msgKind, replyTo, txnID string) (out string, err error) {
	defer c.recoverTo(&err)
	if err := checkAll(
		maxLen("roomID", roomID, maxIDBytes), maxLen("filePath", filePath, maxPathBytes),
		maxLen("mimeType", mimeType, maxIDBytes), maxLen("fileName", fileName, maxFileNameBytes),
		maxLen("caption", caption, maxTextBytes), maxLen("kind", msgKind, maxIDBytes),
		maxLen("replyTo", replyTo, maxIDBytes), maxLen("txnID", txnID, maxIDBytes),
	); err != nil {
		return "", err
	}
	eventID, err := c.eng.SendMedia(roomIDOf(roomID), engine.MediaUpload{
		FilePath: filePath,
		MimeType: mimeType,
		FileName: fileName,
		Caption:  caption,
		Kind:     msgKind,
		ReplyTo:  eventIDOf(replyTo),
		TxnID:    txnID,
	})
	return string(eventID), err
}

// Edit replaces the text of a message.
func (c *Core) Edit(roomID, targetEventID, newText string) (err error) {
	defer c.recoverTo(&err)
	if err := checkAll(
		maxLen("roomID", roomID, maxIDBytes), maxLen("eventID", targetEventID, maxIDBytes),
		maxLen("text", newText, maxTextBytes),
	); err != nil {
		return err
	}
	return c.eng.Edit(roomIDOf(roomID), eventIDOf(targetEventID), newText)
}

// Redact removes a message.
func (c *Core) Redact(roomID, eventID string) (err error) {
	defer c.recoverTo(&err)
	if err := checkAll(maxLen("roomID", roomID, maxIDBytes), maxLen("eventID", eventID, maxIDBytes)); err != nil {
		return err
	}
	return c.eng.Redact(roomIDOf(roomID), eventIDOf(eventID))
}

// React adds a reaction to a message.
func (c *Core) React(roomID, targetEventID, key string) (err error) {
	defer c.recoverTo(&err)
	if err := checkAll(
		maxLen("roomID", roomID, maxIDBytes), maxLen("eventID", targetEventID, maxIDBytes),
		maxLen("key", key, maxKeyBytes),
	); err != nil {
		return err
	}
	return c.eng.React(roomIDOf(roomID), eventIDOf(targetEventID), key)
}

// Unreact removes the user's reaction from a message.
func (c *Core) Unreact(roomID, targetEventID, key string) (err error) {
	defer c.recoverTo(&err)
	if err := checkAll(
		maxLen("roomID", roomID, maxIDBytes), maxLen("eventID", targetEventID, maxIDBytes),
		maxLen("key", key, maxKeyBytes),
	); err != nil {
		return err
	}
	return c.eng.Unreact(roomIDOf(roomID), eventIDOf(targetEventID), key)
}

// MarkRead moves the read position of a chat to eventID.
func (c *Core) MarkRead(roomID, eventID string) (err error) {
	defer c.recoverTo(&err)
	if err := checkAll(maxLen("roomID", roomID, maxIDBytes), maxLen("eventID", eventID, maxIDBytes)); err != nil {
		return err
	}
	return c.eng.MarkRead(roomIDOf(roomID), eventIDOf(eventID))
}

// SetTyping reports whether the user is typing in a chat.
func (c *Core) SetTyping(roomID string, typing bool) (err error) {
	defer c.recoverTo(&err)
	if err := maxLen("roomID", roomID, maxIDBytes); err != nil {
		return err
	}
	return c.eng.SetTyping(roomIDOf(roomID), typing)
}

// SetChatLocalFlags sets {"pinned":bool,"archived":bool,"mutedUntil":int64}. Absent keys are unchanged.
func (c *Core) SetChatLocalFlags(roomID, flagsJSON string) (err error) {
	defer c.recoverTo(&err)
	if err := maxLen("roomID", roomID, maxIDBytes); err != nil {
		return err
	}
	var flags engine.ChatFlags
	if err := json.Unmarshal([]byte(flagsJSON), &flags); err != nil {
		return fmt.Errorf("invalid chat flags: %w", err)
	}
	return c.eng.SetChatLocalFlags(roomIDOf(roomID), flags)
}

// MediaFile returns the absolute local path of an mxc:// URI.
func (c *Core) MediaFile(mxcURI string) (path string, err error) {
	defer c.recoverTo(&err)
	if err := maxLen("mxcURI", mxcURI, maxIDBytes); err != nil {
		return "", err
	}
	return c.eng.MediaFile(mxcURI)
}

// Search returns the JSON []Message whose text contains query, newest first.
func (c *Core) Search(query string, limit int) (out string) {
	defer c.recoverTo(nil)
	if err := maxLen("query", query, maxQueryBytes); err != nil {
		c.report(err)
		return "[]"
	}
	msgs, err := c.eng.Search(query, clampLimit(limit))
	if err != nil {
		c.report(err)
		return "[]"
	}
	return mustJSON(msgs)
}

// Settings returns the JSON CoreSettings.
func (c *Core) Settings() (out string) {
	defer c.recoverTo(nil)
	return mustJSON(c.eng.Settings())
}

// SetSettings stores the JSON CoreSettings.
func (c *Core) SetSettings(settingsJSON string) (err error) {
	defer c.recoverTo(&err)
	var s engine.Settings
	if err := json.Unmarshal([]byte(settingsJSON), &s); err != nil {
		return fmt.Errorf("invalid settings: %w", err)
	}
	return c.eng.SetSettings(s)
}
