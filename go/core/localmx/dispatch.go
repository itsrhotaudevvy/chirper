package localmx

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"time"

	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/networkid"
	"maunium.net/go/mautrix/event"
	"maunium.net/go/mautrix/id"

	"chirper.dev/core/store"
)

// parseContent returns the typed view of content. Content built in Go has Parsed set; content
// that only has Raw fields is round-tripped through JSON.
func parseContent[T any](content *event.Content) (*T, error) {
	if p, ok := content.Parsed.(*T); ok {
		return p, nil
	}
	raw, err := json.Marshal(content)
	if err != nil {
		return nil, fmt.Errorf("failed to marshal content: %w", err)
	}
	var out T
	if err := json.Unmarshal(raw, &out); err != nil {
		return nil, fmt.Errorf("failed to parse content: %w", err)
	}
	return &out, nil
}

// editContentJSON returns the m.new_content of an edit: the explicit raw field when present,
// otherwise the NewContent of the parsed edit, otherwise the edit content itself.
func editContentJSON(content *event.Content, msg *event.MessageEventContent) (string, error) {
	var v any = msg
	if raw, ok := content.Raw["m.new_content"]; ok {
		v = raw
	} else if msg.NewContent != nil {
		v = msg.NewContent
	}
	b, err := json.Marshal(v)
	if err != nil {
		return "", fmt.Errorf("failed to marshal edit content: %w", err)
	}
	return string(b), nil
}

// eventIDFor picks the ID of a locally originated event: deterministic when the bridge provides
// the message it belongs to, random otherwise.
func (c *Connector) eventIDFor(roomID id.RoomID, extra *bridgev2.MatrixSendExtra) id.EventID {
	if extra != nil && extra.MessageMeta != nil {
		return c.GenerateDeterministicEventID(roomID, networkid.PortalKey{}, extra.MessageMeta.ID, extra.MessageMeta.PartID)
	}
	return newLocalEventID()
}

func nowMS() int64 {
	return time.Now().UnixMilli()
}

// arrival says how a remote event reached the bridge.
type arrival int

const (
	// arrivedLive is an event delivered in real time through intent.SendMessage. The sink is
	// notified for each event.
	arrivedLive arrival = iota
	// arrivedBackfill is history stored through BatchSend. The sink is notified by the batch.
	arrivedBackfill
)

// liveWindow is how old an event may be when it is inserted and still count as live.
const liveWindow = 2 * time.Minute

// dispatch stores one event sent through an intent and returns its event ID. Relations change
// existing rows: m.replace edits the target, m.annotation adds a reaction, and redactions remove
// content or reactions. Other message events are stored as timeline rows. Only a live message
// that is inserted now increments the unread count and is reported as live to the sink.
func (c *Connector) dispatch(ctx context.Context, roomID id.RoomID, sender id.UserID, eventType event.Type,
	content *event.Content, eventID id.EventID, ts int64, txnID string, how arrival,
) (id.EventID, error) {
	notify := how == arrivedLive
	switch {
	case eventType.IsState():
		return "", fmt.Errorf("state event %s must be sent with SendState", eventType.Type)
	case eventType == event.EventRedaction:
		return eventID, c.redact(ctx, roomID, content, notify)
	case eventType == event.EventReaction:
		return eventID, c.addReaction(ctx, roomID, sender, eventID, content, ts, notify)
	}
	if eventType == event.EventMessage || eventType == event.EventSticker {
		msg, err := parseContent[event.MessageEventContent](content)
		if err != nil {
			return "", err
		}
		if rel := msg.RelatesTo; rel != nil && rel.Type == event.RelReplace {
			return eventID, c.applyEdit(ctx, roomID, rel.EventID, content, msg, ts, notify)
		}
	}
	raw, err := json.Marshal(content)
	if err != nil {
		return "", fmt.Errorf("failed to marshal event content: %w", err)
	}
	ev := &store.Event{
		EventID:     eventID,
		RoomID:      roomID,
		Sender:      sender,
		Type:        eventType.Type,
		ContentJSON: string(raw),
		TS:          ts,
		TxnID:       txnID,
	}
	inserted, err := c.st.InsertEvent(ctx, ev)
	if err != nil {
		return "", err
	}
	live := inserted && notify && nowMS()-ts < liveWindow.Milliseconds()
	if live {
		if err := c.st.IncrementUnread(ctx, eventID); err != nil {
			return "", err
		}
	}
	if notify {
		c.sink.RoomChanged(roomID)
		c.sink.EventChanged(roomID, eventID, live)
	}
	return eventID, nil
}

func (c *Connector) redact(ctx context.Context, roomID id.RoomID, content *event.Content, notify bool) error {
	red, err := parseContent[event.RedactionEventContent](content)
	if err != nil {
		return err
	}
	res, err := c.st.Redact(ctx, red.Redacts)
	if errors.Is(err, store.ErrNotFound) {
		// Redacting an event that was never stored is not an error for the bridge.
		return nil
	} else if err != nil {
		return err
	}
	// A redaction that arrives from the network is final, so the media of the message can go.
	if err := c.st.ReleaseEventMedia(ctx, red.Redacts); err != nil {
		return err
	}
	if !notify {
		return nil
	}
	if res.IsReaction {
		c.sink.EventRemoved(res.RoomID, res.EventID)
	}
	c.sink.EventChanged(res.RoomID, res.TargetEventID, false)
	c.sink.RoomChanged(roomID)
	return nil
}

func (c *Connector) addReaction(ctx context.Context, roomID id.RoomID, sender id.UserID, eventID id.EventID,
	content *event.Content, ts int64, notify bool,
) error {
	rc, err := parseContent[event.ReactionEventContent](content)
	if err != nil {
		return err
	}
	_, err = c.st.AddReaction(ctx, &store.Reaction{
		EventID:       eventID,
		RoomID:        roomID,
		TargetEventID: rc.RelatesTo.EventID,
		Sender:        sender,
		Key:           rc.RelatesTo.Key,
		TS:            ts,
	})
	if err != nil {
		return err
	}
	if notify {
		c.sink.EventChanged(roomID, rc.RelatesTo.EventID, false)
	}
	return nil
}

func (c *Connector) applyEdit(ctx context.Context, roomID id.RoomID, target id.EventID, content *event.Content,
	msg *event.MessageEventContent, ts int64, notify bool,
) error {
	newContent, err := editContentJSON(content, msg)
	if err != nil {
		return err
	}
	if err := c.st.ApplyEdit(ctx, roomID, target, newContent, ts); err != nil {
		return fmt.Errorf("failed to apply edit to %s: %w", target, err)
	}
	if notify {
		c.sink.EventChanged(roomID, target, false)
	}
	return nil
}
