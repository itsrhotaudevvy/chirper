// Package localmx implements bridgev2's MatrixConnector without a Matrix homeserver. Bridged
// rooms, members and events are written to the local store instead of being sent to a server.
package localmx

import (
	"context"
	"errors"
	"fmt"
	"os"
	"sync"
	"time"

	"github.com/rs/zerolog"
	"maunium.net/go/mautrix"
	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/networkid"
	"maunium.net/go/mautrix/bridgev2/status"
	"maunium.net/go/mautrix/event"
	"maunium.net/go/mautrix/id"

	"chirper.dev/core/store"
)

// ErrNotSupported is returned for Matrix features that have no local equivalent yet.
var ErrNotSupported = errors.New("not supported yet")

// EventSink receives change notifications; implemented by the engine. Must not block.
type EventSink interface {
	RoomChanged(roomID id.RoomID)
	// EventChanged reports a stored or updated event. live is true only for a message that arrived
	// in real time and was inserted now; history, state changes and updates report false.
	EventChanged(roomID id.RoomID, eventID id.EventID, live bool)
	EventRemoved(roomID id.RoomID, eventID id.EventID)
	Typing(roomID id.RoomID, userIDs []id.UserID)
	BridgeState(bridgeID networkid.BridgeID, state status.BridgeState)
	MessageStatus(roomID id.RoomID, eventID id.EventID, st *bridgev2.MessageStatus)
}

// errorReporter is optionally implemented by an EventSink to receive failures of the connector's
// background tasks, such as a recovered panic in a timer. It must not block.
type errorReporter interface {
	ReportError(err error)
}

// Connector is the in-process MatrixConnector of one bridge (one network).
type Connector struct {
	bridgeID  networkid.BridgeID
	st        *store.Store
	mediaDir  string
	sink      EventSink
	caps      bridgev2.MatrixCapabilities
	typingMu  sync.Mutex
	typing    map[id.RoomID]map[id.UserID]time.Time
	profileMu sync.Mutex
}

var _ bridgev2.MatrixConnector = (*Connector)(nil)

// New creates the connector for bridgeID. Media bytes are written to mediaDir.
func New(bridgeID networkid.BridgeID, st *store.Store, mediaDir string, sink EventSink) *Connector {
	return &Connector{
		bridgeID: bridgeID,
		st:       st,
		mediaDir: mediaDir,
		sink:     sink,
		caps: bridgev2.MatrixCapabilities{
			AutoJoinInvites:       true,
			BatchSending:          true,
			ArbitraryMemberChange: true,
			ExtraProfileMeta:      true,
		},
		typing: make(map[id.RoomID]map[id.UserID]time.Time),
	}
}

// Init implements bridgev2.MatrixConnector. The connector keeps no bridge references.
func (c *Connector) Init(*bridgev2.Bridge) {}

// Start creates the media directory.
func (c *Connector) Start(context.Context) error {
	if err := os.MkdirAll(c.mediaDir, 0o700); err != nil {
		return fmt.Errorf("failed to create media directory: %w", err)
	}
	return nil
}

// PreStop implements bridgev2.MatrixConnector.
func (c *Connector) PreStop() {}

// Stop implements bridgev2.MatrixConnector.
func (c *Connector) Stop() {}

// GetCapabilities implements bridgev2.MatrixConnector.
func (c *Connector) GetCapabilities() *bridgev2.MatrixCapabilities {
	return &c.caps
}

// BotIntent returns the intent of the bridge bot. It is required by bridgev2.NewBridge.
func (c *Connector) BotIntent() bridgev2.MatrixAPI {
	return c.newIntent(c.botUserID(), false)
}

// GhostIntent implements bridgev2.MatrixConnector.
func (c *Connector) GhostIntent(userID networkid.UserID) bridgev2.MatrixAPI {
	return c.newIntent(c.FormatGhostMXID(userID), false)
}

// NewUserIntent implements bridgev2.MatrixConnector. Only the local user can double-puppet; any
// token is accepted and reported back as "local", so bridgev2 attributes the user's messages to @me.
func (c *Connector) NewUserIntent(_ context.Context, userID id.UserID, _ string) (bridgev2.MatrixAPI, string, error) {
	if userID != store.SelfUserID {
		return nil, "", fmt.Errorf("%w: double puppeting %s", ErrNotSupported, userID)
	}
	return c.newIntent(userID, true), "local", nil
}

// SendBridgeStatus implements bridgev2.MatrixConnector.
func (c *Connector) SendBridgeStatus(_ context.Context, state *status.BridgeState) error {
	c.sink.BridgeState(c.bridgeID, *state)
	return nil
}

// SendMessageStatus records the outcome of a Matrix event sent by the local user.
func (c *Connector) SendMessageStatus(ctx context.Context, ms *bridgev2.MessageStatus, evt *bridgev2.MessageStatusEventInfo) {
	if evt == nil || evt.SourceEventID == "" {
		return
	}
	var state, errText string
	switch ms.Status {
	case event.MessageStatusSuccess:
		state = "sent"
	case event.MessageStatusPending:
		state = "sending"
	default:
		state = "failed"
		errText = firstNonEmpty(ms.Message, string(ms.ErrorReason))
	}
	if err := c.st.SetEventStatus(ctx, evt.SourceEventID, state, errText); err != nil {
		zerolog.Ctx(ctx).Err(err).Stringer("event_id", evt.SourceEventID).Msg("Failed to store message status")
	}
	c.sink.MessageStatus(evt.RoomID, evt.SourceEventID, ms)
}

func firstNonEmpty(values ...string) string {
	for _, v := range values {
		if v != "" {
			return v
		}
	}
	return ""
}

// GenerateContentURI implements bridgev2.MatrixConnector.
func (c *Connector) GenerateContentURI(context.Context, networkid.MediaID) (id.ContentURIString, error) {
	return "", fmt.Errorf("%w: direct media", ErrNotSupported)
}

// ParseContentURI implements bridgev2.MatrixConnector.
func (c *Connector) ParseContentURI(_ context.Context, contentURI id.ContentURIString) (networkid.MediaID, error) {
	parsed, err := contentURI.Parse()
	if err != nil {
		return nil, err
	}
	if parsed.Homeserver != store.ServerName {
		return nil, fmt.Errorf("content URI %s is not local", contentURI)
	}
	return networkid.MediaID(parsed.FileID), nil
}

// GetPowerLevels returns the default levels: the local user is admin, everyone else is a member.
func (c *Connector) GetPowerLevels(context.Context, id.RoomID) (*event.PowerLevelsEventContent, error) {
	return &event.PowerLevelsEventContent{
		Events: map[string]int{},
		Users:  map[id.UserID]int{store.SelfUserID: 100},
	}, nil
}

// GetMembers implements bridgev2.MatrixConnector.
func (c *Connector) GetMembers(ctx context.Context, roomID id.RoomID) (map[id.UserID]*event.MemberEventContent, error) {
	members, err := c.st.ListMembers(ctx, roomID)
	if err != nil {
		return nil, err
	}
	out := make(map[id.UserID]*event.MemberEventContent, len(members))
	for _, m := range members {
		out[m.UserID] = memberContent(&m)
	}
	return out, nil
}

// GetMemberInfo returns the member content, or nil if the user is not a member row of the room.
func (c *Connector) GetMemberInfo(ctx context.Context, roomID id.RoomID, userID id.UserID) (*event.MemberEventContent, error) {
	m, err := c.st.GetMember(ctx, roomID, userID)
	if errors.Is(err, store.ErrNotFound) {
		return nil, nil
	} else if err != nil {
		return nil, err
	}
	return memberContent(m), nil
}

func memberContent(m *store.Member) *event.MemberEventContent {
	return &event.MemberEventContent{
		Membership:  m.Membership,
		Displayname: m.Displayname,
		AvatarURL:   m.AvatarURL,
	}
}

// BatchSend stores a batch of backfilled events in order. Events carry their deterministic IDs.
// Backfilled events never count as unread and are never live. The sink is notified once for the
// room and once for the last event, not per event.
func (c *Connector) BatchSend(ctx context.Context, roomID id.RoomID, req *mautrix.ReqBeeperBatchSend, extras []*bridgev2.MatrixSendExtra) (*mautrix.RespBeeperBatchSend, error) {
	resp := &mautrix.RespBeeperBatchSend{EventIDs: make([]id.EventID, 0, len(req.Events))}
	for i, evt := range req.Events {
		var extra *bridgev2.MatrixSendExtra
		if i < len(extras) {
			extra = extras[i]
		}
		eventID := evt.ID
		if eventID == "" {
			eventID = c.eventIDFor(roomID, extra)
		}
		ts := evt.Timestamp
		if ts == 0 {
			ts = nowMS()
		}
		stored, err := c.dispatch(ctx, roomID, evt.Sender, evt.Type, &evt.Content, eventID, ts, "", arrivedBackfill)
		if err != nil {
			return nil, fmt.Errorf("failed to store batch event %d: %w", i, err)
		}
		resp.EventIDs = append(resp.EventIDs, stored)
	}
	c.sink.RoomChanged(roomID)
	if n := len(resp.EventIDs); n > 0 {
		c.sink.EventChanged(roomID, resp.EventIDs[n-1], false)
	}
	return resp, nil
}

// ServerName implements bridgev2.MatrixConnector.
func (c *Connector) ServerName() string {
	return store.ServerName
}
