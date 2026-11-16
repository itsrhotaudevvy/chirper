package engine

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"sync"
	"time"

	"github.com/google/uuid"
	"go.mau.fi/util/variationselector"
	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/event"
	"maunium.net/go/mautrix/id"

	"chirper.dev/core/localmx"
	"chirper.dev/core/store"
)

// loadOlderTimeout bounds how long LoadOlder waits for the network.
const loadOlderTimeout = 25 * time.Second

// syncHistoryTimeout bounds how long SyncHistory spends listing the chats of one network.
const syncHistoryTimeout = 30 * time.Second

// sendingStatus is the local status of a message the network has not confirmed yet.
const sendingStatus = "sending"

// normalizeKey removes variation selectors so that emoji keys compare equal on both sides.
func normalizeKey(key string) string {
	return variationselector.Remove(key)
}

// newLocalEventID returns the ID of an event that the network has not confirmed yet.
func newLocalEventID() id.EventID {
	return id.EventID("$local." + uuid.NewString())
}

// SendText sends a text message, optionally as a reply to replyTo. It returns the local event ID.
func (e *Engine) SendText(roomID id.RoomID, text string, replyTo id.EventID, txnID string) (id.EventID, error) {
	rf, err := e.resolveRoom(e.ctx, roomID)
	if err != nil {
		return "", err
	}
	content := &event.MessageEventContent{MsgType: event.MsgText, Body: text}
	setReply(content, replyTo)
	return e.sendMessage(rf, event.EventMessage, content, txnID)
}

// setReply makes content a reply to replyTo when replyTo is set.
func setReply(content *event.MessageEventContent, replyTo id.EventID) {
	if replyTo != "" {
		content.RelatesTo = (&event.RelatesTo{}).SetReplyTo(replyTo)
	}
}

// sendMessage stores the message as sending, then hands it to the bridge. A message the bridge
// rejects at once is marked failed.
func (e *Engine) sendMessage(rf roomRef, evtType event.Type, content *event.MessageEventContent, txnID string) (id.EventID, error) {
	ctx := e.ctx
	raw, err := json.Marshal(content)
	if err != nil {
		return "", fmt.Errorf("failed to encode message: %w", err)
	}
	eventID := newLocalEventID()
	now := time.Now().UnixMilli()
	inserted, err := e.st.InsertEvent(ctx, &store.Event{
		EventID:     eventID,
		RoomID:      rf.room.RoomID,
		Sender:      store.SelfUserID,
		Type:        evtType.Type,
		ContentJSON: string(raw),
		TS:          now,
		TxnID:       txnID,
		Status:      sendingStatus,
	})
	if err != nil {
		return "", err
	}
	if !inserted {
		return "", fmt.Errorf("message with transaction %q was already sent", txnID)
	}
	e.queueMessage(eventID, false)
	e.queueChat(rf.room.RoomID)
	res := rf.inst.br.QueueMatrixEvent(ctx, &event.Event{
		Type:      evtType,
		RoomID:    rf.room.RoomID,
		Sender:    store.SelfUserID,
		ID:        eventID,
		Timestamp: now,
		Content:   event.Content{Parsed: content},
	})
	if !res.Success {
		if err := e.st.SetEventStatus(ctx, eventID, "failed", rejectionText(res)); err != nil {
			e.log.Error().Err(err).Msg("Failed to mark message failed")
		}
		e.queueMessage(eventID, false)
	}
	return eventID, nil
}

// rejectionText is the error text of an event the bridge did not accept.
func rejectionText(res bridgev2.EventHandlingResult) string {
	if res.Error != nil {
		return res.Error.Error()
	}
	return "not accepted by the network bridge"
}

// queueWithUndo queues an event whose optimistic write is recorded under eventID. The write is
// undone if the bridge rejects the event now or the network later reports a failure.
func (e *Engine) queueWithUndo(rf roomRef, evt *event.Event, op *undoOp) error {
	e.pending.add(evt.ID, op)
	res := rf.inst.br.QueueMatrixEvent(e.ctx, evt)
	if res.Success {
		return nil
	}
	if op := e.pending.take(evt.ID); op != nil {
		e.undo(op)
	}
	return fmt.Errorf("network rejected the change: %s", rejectionText(res))
}

// messageIn returns the stored text or media message eventID of roomID that can still be changed.
func (e *Engine) messageIn(roomID id.RoomID, eventID id.EventID) (*store.Event, error) {
	ev, err := e.st.GetEvent(e.ctx, eventID)
	if err != nil {
		return nil, fmt.Errorf("unknown message %s: %w", eventID, err)
	}
	if ev.RoomID != roomID || ev.StateKey != nil || ev.Type != "m.room.message" || ev.Redacted {
		return nil, fmt.Errorf("message %s cannot be changed", eventID)
	}
	return ev, nil
}

// SendMedia uploads a local file into the media store and sends it as a message. The file is
// checked before the chat is resolved, and only its content and sanitised metadata are used.
func (e *Engine) SendMedia(roomID id.RoomID, up MediaUpload) (id.EventID, error) {
	info, err := checkUploadFile(up.FilePath)
	if err != nil {
		return "", err
	}
	rf, err := e.resolveRoom(e.ctx, roomID)
	if err != nil {
		return "", err
	}
	mime := safeMime(up.MimeType, up.FilePath)
	name := safeFileName(up.FileName, up.FilePath)
	uri, _, err := rf.inst.br.Bot.UploadMediaStream(e.ctx, roomID, info.Size(), false,
		func(w io.Writer) (*bridgev2.FileStreamResult, error) {
			f, err := openUploadFile(up.FilePath, info)
			if err != nil {
				return nil, err
			}
			defer f.Close()
			if _, err := io.CopyN(w, f, info.Size()); err != nil {
				return nil, errors.New("failed to read file")
			}
			return &bridgev2.FileStreamResult{FileName: name, MimeType: mime}, nil
		})
	if err != nil {
		return "", fmt.Errorf("failed to store media: %w", err)
	}
	content, evtType, err := mediaContent(up, uri, info.Size(), mime, name)
	if err != nil {
		return "", err
	}
	setReply(content, up.ReplyTo)
	return e.sendMessage(rf, evtType, content, up.TxnID)
}

// Edit replaces the text of an outgoing or incoming message the user may change.
func (e *Engine) Edit(roomID id.RoomID, target id.EventID, newText string) error {
	rf, err := e.resolveRoom(e.ctx, roomID)
	if err != nil {
		return err
	}
	snap, err := e.messageIn(roomID, target)
	if err != nil {
		return err
	}
	edit := event.MessageEventContent{MsgType: event.MsgText, Body: newText}
	edit.SetEdit(target)
	newJSON, err := json.Marshal(edit.NewContent)
	if err != nil {
		return fmt.Errorf("failed to encode edit: %w", err)
	}
	if err := e.st.ApplyEdit(e.ctx, roomID, target, string(newJSON), time.Now().UnixMilli()); err != nil {
		return err
	}
	e.queueMessage(target, false)
	editID := newLocalEventID()
	return e.queueWithUndo(rf, &event.Event{
		Type: event.EventMessage, RoomID: roomID, Sender: store.SelfUserID, ID: editID,
		Timestamp: time.Now().UnixMilli(), Content: event.Content{Parsed: &edit},
	}, &undoOp{roomID: roomID, target: target, undo: func(ctx context.Context) error {
		return e.st.RestoreEvent(ctx, snap)
	}})
}

// Redact removes a message the user sent.
func (e *Engine) Redact(roomID id.RoomID, eventID id.EventID) error {
	rf, err := e.resolveRoom(e.ctx, roomID)
	if err != nil {
		return err
	}
	snap, err := e.messageIn(roomID, eventID)
	if err != nil {
		return err
	}
	if _, err := e.st.Redact(e.ctx, eventID); err != nil {
		return err
	}
	e.queueMessage(eventID, false)
	e.queueChat(roomID)
	redactID := newLocalEventID()
	return e.queueWithUndo(rf, &event.Event{
		Type: event.EventRedaction, RoomID: roomID, Sender: store.SelfUserID, ID: redactID,
		Timestamp: time.Now().UnixMilli(), Content: event.Content{Parsed: &event.RedactionEventContent{Redacts: eventID}},
	}, &undoOp{roomID: roomID, target: eventID, undo: func(ctx context.Context) error {
		return e.st.RestoreEvent(ctx, snap)
	}, finish: func(ctx context.Context) error {
		return e.st.ReleaseEventMedia(ctx, eventID)
	}})
}

// React adds the user's reaction key to the message target. Adding a reaction that exists is a no-op.
func (e *Engine) React(roomID id.RoomID, target id.EventID, key string) error {
	rf, err := e.resolveRoom(e.ctx, roomID)
	if err != nil {
		return err
	}
	if _, err := e.st.GetEvent(e.ctx, target); err != nil {
		return fmt.Errorf("unknown message %s: %w", target, err)
	}
	reactID := newLocalEventID()
	now := time.Now().UnixMilli()
	inserted, err := e.st.AddReaction(e.ctx, &store.Reaction{
		EventID: reactID, RoomID: roomID, TargetEventID: target, Sender: store.SelfUserID,
		Key: normalizeKey(key), TS: now,
	})
	if err != nil || !inserted {
		return err
	}
	e.queueMessage(target, false)
	return e.queueWithUndo(rf, &event.Event{
		Type: event.EventReaction, RoomID: roomID, Sender: store.SelfUserID, ID: reactID, Timestamp: now,
		Content: event.Content{Parsed: &event.ReactionEventContent{
			RelatesTo: event.RelatesTo{Type: event.RelAnnotation, EventID: target, Key: key},
		}},
	}, &undoOp{roomID: roomID, target: target, undo: func(ctx context.Context) error {
		_, err := e.st.RemoveReactionByEventID(ctx, reactID)
		if errors.Is(err, store.ErrNotFound) {
			return nil
		}
		return err
	}})
}

// Unreact removes the user's reaction key from the message target.
func (e *Engine) Unreact(roomID id.RoomID, target id.EventID, key string) error {
	rf, err := e.resolveRoom(e.ctx, roomID)
	if err != nil {
		return err
	}
	reactions, err := e.st.FindReactions(e.ctx, target, store.SelfUserID)
	if err != nil {
		return err
	}
	want := normalizeKey(key)
	var mine *store.Reaction
	for i := range reactions {
		if normalizeKey(reactions[i].Key) == want {
			mine = &reactions[i]
			break
		}
	}
	if mine == nil {
		return fmt.Errorf("no %q reaction on message %s", key, target)
	}
	if _, err := e.st.RemoveReactionByEventID(e.ctx, mine.EventID); err != nil {
		return err
	}
	e.queueMessage(target, false)
	redactID := newLocalEventID()
	removed := *mine
	return e.queueWithUndo(rf, &event.Event{
		Type: event.EventRedaction, RoomID: roomID, Sender: store.SelfUserID, ID: redactID,
		Timestamp: time.Now().UnixMilli(), Content: event.Content{Parsed: &event.RedactionEventContent{Redacts: mine.EventID}},
	}, &undoOp{roomID: roomID, target: target, undo: func(ctx context.Context) error {
		_, err := e.st.AddReaction(ctx, &removed)
		return err
	}})
}

// MarkRead moves the read position of roomID to eventID. The network is told only when read
// receipts are enabled for it.
func (e *Engine) MarkRead(roomID id.RoomID, eventID id.EventID) error {
	rf, err := e.resolveRoom(e.ctx, roomID)
	if err != nil {
		return err
	}
	if err := e.st.MarkReadUpTo(e.ctx, roomID, eventID); err != nil {
		return err
	}
	e.queueChat(roomID)
	if !allows(e.currentSettings().SendReadReceipts, rf.inst.spec.id) {
		return nil
	}
	receipts := event.ReceiptEventContent{}
	receipts.Set(eventID, event.ReceiptTypeRead, store.SelfUserID, event.ReadReceipt{Timestamp: time.Now()})
	rf.inst.br.QueueMatrixEvent(e.ctx, &event.Event{
		Type: event.EphemeralEventReceipt, RoomID: roomID, Sender: store.SelfUserID,
		Content: event.Content{Parsed: &receipts},
		Mautrix: event.MautrixInfo{EventSource: event.SourceEphemeral},
	})
	return nil
}

// SetTyping tells the network that the user is typing in roomID, when typing notices are enabled.
func (e *Engine) SetTyping(roomID id.RoomID, typing bool) error {
	rf, err := e.resolveRoom(e.ctx, roomID)
	if err != nil {
		return err
	}
	if !allows(e.currentSettings().SendTyping, rf.inst.spec.id) {
		return nil
	}
	users := []id.UserID{}
	if typing {
		users = append(users, store.SelfUserID)
	}
	rf.inst.br.QueueMatrixEvent(e.ctx, &event.Event{
		Type: event.EphemeralEventTyping, RoomID: roomID, Sender: store.SelfUserID,
		Content: event.Content{Parsed: &event.TypingEventContent{UserIDs: users}},
		Mautrix: event.MautrixInfo{EventSource: event.SourceEphemeral},
	})
	return nil
}

// LoadOlder asks the network for older history of roomID and waits up to loadOlderTimeout for it.
// It reports whether more history may be available; false means the beginning of the history.
func (e *Engine) LoadOlder(roomID id.RoomID) (bool, error) {
	rf, err := e.resolveRoom(e.ctx, roomID)
	if err != nil {
		return false, err
	}
	portal, err := rf.inst.br.GetPortalByMXID(e.ctx, roomID)
	if err != nil {
		return false, fmt.Errorf("failed to load chat: %w", err)
	}
	if portal == nil {
		return false, errors.New("chat has no portal")
	}
	pending, err := e.backfillPending(rf, portal)
	if err != nil || !pending {
		return false, err
	}
	if err := e.waitBackfill(rf, portal); err != nil {
		return true, err
	}
	return e.backfillPending(rf, portal)
}

// backfillPending reports whether the network has older history of portal left to fetch.
func (e *Engine) backfillPending(rf roomRef, portal *bridgev2.Portal) (bool, error) {
	task, err := rf.inst.br.DB.BackfillTask.GetNextForPortal(e.ctx, portal.PortalKey, false)
	if err != nil {
		return false, fmt.Errorf("failed to read backfill state: %w", err)
	}
	return task != nil, nil
}

// waitBackfill runs the backfill of portal and waits for its completion.
func (e *Engine) waitBackfill(rf roomRef, portal *bridgev2.Portal) error {
	done := make(chan error, 1)
	var once sync.Once
	rf.inst.br.WakeupBackfillQueue(&bridgev2.ManualBackfill{
		Source: rf.login,
		Portal: portal,
		DoneCallback: func(err error) {
			once.Do(func() { done <- err })
		},
	})
	select {
	case err := <-done:
		return err
	case <-time.After(loadOlderTimeout):
		return fmt.Errorf("backfill did not complete within %s", loadOlderTimeout)
	case <-e.ctx.Done():
		return e.ctx.Err()
	}
}

// MarkUnread sets whether roomID is marked unread. Marking it read moves the read position to the
// newest message. Like MarkRead, the change stays on this device.
func (e *Engine) MarkUnread(roomID id.RoomID, unread bool) error {
	room, err := e.st.GetRoom(e.ctx, roomID)
	if errors.Is(err, store.ErrNotFound) || (err == nil && room.Deleted) {
		return fmt.Errorf("unknown chat %s", roomID)
	} else if err != nil {
		return err
	}
	if err := e.st.MarkUnread(e.ctx, roomID, unread); err != nil {
		return err
	}
	e.queueChat(roomID)
	return nil
}

// SetChatLocalFlags changes the local-only flags of a chat. They are never sent to the network.
func (e *Engine) SetChatLocalFlags(roomID id.RoomID, flags ChatFlags) error {
	if err := e.st.SetRoomFields(e.ctx, roomID, store.RoomFields{
		Pinned: flags.Pinned, Archived: flags.Archived, MutedUntil: flags.MutedUntil,
	}); err != nil {
		return err
	}
	e.queueChat(roomID)
	return nil
}

// MediaFile returns the local path of a media URI of this device. The media must be known to the
// store, and the path is resolved inside the media directory.
func (e *Engine) MediaFile(mxc string) (string, error) {
	uri, err := id.ContentURIString(mxc).Parse()
	if err != nil {
		return "", fmt.Errorf("invalid media URI: %w", err)
	}
	if uri.Homeserver != store.ServerName {
		return "", fmt.Errorf("media %s is not stored on this device", mxc)
	}
	path, err := localmx.ResolveMediaFile(e.mediaDir, uri.FileID)
	if err != nil {
		return "", err
	}
	if _, err := e.st.GetMedia(e.ctx, uri.FileID); err != nil {
		return "", fmt.Errorf("media not found: %w", err)
	}
	return path, nil
}
