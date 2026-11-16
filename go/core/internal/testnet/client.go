package testnet

import (
	"context"
	"fmt"
	"sync/atomic"
	"time"

	"github.com/rs/zerolog"
	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/database"
	"maunium.net/go/mautrix/bridgev2/networkid"
	"maunium.net/go/mautrix/bridgev2/simplevent"
	"maunium.net/go/mautrix/bridgev2/status"
	"maunium.net/go/mautrix/event"
)

// replyDelay is how long the fake contact takes to answer a message from the local user.
const replyDelay = 300 * time.Millisecond

// Client is the NetworkAPI of one fake login.
type Client struct {
	login  *bridgev2.UserLogin
	name   string
	nextID atomic.Int64
}

var (
	_ bridgev2.NetworkAPI                    = (*Client)(nil)
	_ bridgev2.BackfillingNetworkAPI         = (*Client)(nil)
	_ bridgev2.EditHandlingNetworkAPI        = (*Client)(nil)
	_ bridgev2.ReactionHandlingNetworkAPI    = (*Client)(nil)
	_ bridgev2.RedactionHandlingNetworkAPI   = (*Client)(nil)
	_ bridgev2.ReadReceiptHandlingNetworkAPI = (*Client)(nil)
	_ bridgev2.TypingHandlingNetworkAPI      = (*Client)(nil)
)

func newClient(login *bridgev2.UserLogin, name string) *Client {
	return &Client{login: login, name: name}
}

// Connect implements bridgev2.NetworkAPI. The seed dataset is queued in the background; it is
// idempotent, so reconnecting does not duplicate anything.
func (c *Client) Connect(context.Context) {
	c.login.BridgeState.Send(status.BridgeState{StateEvent: status.StateConnected})
	go c.seed(c.login.Log.WithContext(c.login.Bridge.BackgroundCtx))
}

// Disconnect implements bridgev2.NetworkAPI.
func (c *Client) Disconnect() {}

// IsLoggedIn implements bridgev2.NetworkAPI.
func (c *Client) IsLoggedIn() bool { return true }

// LogoutRemote implements bridgev2.NetworkAPI.
func (c *Client) LogoutRemote(context.Context) {}

// IsThisUser implements bridgev2.NetworkAPI.
func (c *Client) IsThisUser(_ context.Context, userID networkid.UserID) bool {
	return userID == selfID
}

// GetChatInfo implements bridgev2.NetworkAPI.
func (c *Client) GetChatInfo(_ context.Context, portal *bridgev2.Portal) (*bridgev2.ChatInfo, error) {
	for _, chat := range seedChats {
		if chat.key.ID == portal.PortalKey.ID {
			return chatInfo(chat), nil
		}
	}
	return nil, fmt.Errorf("unknown demo chat %q", portal.PortalKey.ID)
}

// GetUserInfo implements bridgev2.NetworkAPI.
func (c *Client) GetUserInfo(_ context.Context, ghost *bridgev2.Ghost) (*bridgev2.UserInfo, error) {
	if ghost.ID == selfID {
		return &bridgev2.UserInfo{Identifiers: []string{string(selfID)}, Name: &c.name}, nil
	}
	u, ok := users[ghost.ID]
	if !ok {
		return nil, fmt.Errorf("unknown demo user %q", ghost.ID)
	}
	return &bridgev2.UserInfo{Identifiers: []string{string(u.id)}, Name: &u.name}, nil
}

// GetCapabilities implements bridgev2.NetworkAPI. Everything the fake network handles is fully supported.
func (c *Client) GetCapabilities(context.Context, *bridgev2.Portal) *event.RoomFeatures {
	return &event.RoomFeatures{
		ID:       "demo-v1",
		Reply:    event.CapLevelFullySupported,
		Edit:     event.CapLevelFullySupported,
		Delete:   event.CapLevelFullySupported,
		Reaction: event.CapLevelFullySupported,
		File:     fileFeatures(),
	}
}

// fileFeatures are the media types the fake network accepts: common image, video, audio and
// document formats, with captions.
func fileFeatures() event.FileFeatureMap {
	const maxSize = 10 * 1024 * 1024
	types := map[string]event.CapabilitySupportLevel{
		"image/png":                event.CapLevelFullySupported,
		"image/jpeg":               event.CapLevelFullySupported,
		"image/gif":                event.CapLevelFullySupported,
		"video/mp4":                event.CapLevelFullySupported,
		"audio/mpeg":               event.CapLevelFullySupported,
		"application/pdf":          event.CapLevelFullySupported,
		"application/octet-stream": event.CapLevelFullySupported,
	}
	features := &event.FileFeatures{MimeTypes: types, Caption: event.CapLevelFullySupported, MaxSize: maxSize}
	out := event.FileFeatureMap{}
	for _, msgType := range []event.CapabilityMsgType{
		event.MsgImage, event.MsgVideo, event.MsgAudio, event.MsgFile, event.CapMsgVoice, event.CapMsgSticker,
	} {
		out[msgType] = features.Clone()
	}
	return out
}

// HandleMatrixMessage implements bridgev2.NetworkAPI. The message is accepted and the fake
// contact replies a moment later.
func (c *Client) HandleMatrixMessage(_ context.Context, msg *bridgev2.MatrixMessage) (*bridgev2.MatrixMessageResponse, error) {
	id := networkid.MessageID(fmt.Sprintf("out-%d", c.nextID.Add(1)))
	key := msg.Portal.PortalKey
	text := msg.Content.Body
	time.AfterFunc(replyDelay, func() { c.reply(key, id, text) })
	return &bridgev2.MatrixMessageResponse{
		DB: &database.Message{ID: id, Timestamp: time.UnixMilli(msg.Event.Timestamp)},
	}, nil
}

// reply sends the fake contact's answer to a message: a text reply and a thumbs-up reaction.
func (c *Client) reply(key networkid.PortalKey, target networkid.MessageID, text string) {
	ctx := c.login.Log.WithContext(c.login.Bridge.BackgroundCtx)
	now := time.Now()
	alice := bridgev2.EventSender{Sender: "alice"}
	c.queue(ctx, newTextMessage(key, alice, networkid.MessageID(fmt.Sprintf("reply-%d", c.nextID.Add(1))),
		"echo: "+text, now))
	c.queue(ctx, &simplevent.Reaction{
		EventMeta:     simplevent.EventMeta{Type: bridgev2.RemoteEventReaction, PortalKey: key, Sender: alice, Timestamp: now},
		TargetMessage: target,
		EmojiID:       "👍",
		Emoji:         "👍",
	})
}

// PreHandleMatrixReaction implements bridgev2.ReactionHandlingNetworkAPI.
func (c *Client) PreHandleMatrixReaction(_ context.Context, msg *bridgev2.MatrixReaction) (bridgev2.MatrixReactionPreResponse, error) {
	key := msg.Content.RelatesTo.Key
	return bridgev2.MatrixReactionPreResponse{SenderID: selfID, EmojiID: networkid.EmojiID(key), Emoji: key}, nil
}

// HandleMatrixReaction implements bridgev2.ReactionHandlingNetworkAPI.
func (c *Client) HandleMatrixReaction(context.Context, *bridgev2.MatrixReaction) (*database.Reaction, error) {
	return &database.Reaction{}, nil
}

// HandleMatrixReactionRemove implements bridgev2.ReactionHandlingNetworkAPI.
func (c *Client) HandleMatrixReactionRemove(context.Context, *bridgev2.MatrixReactionRemove) error {
	return nil
}

// HandleMatrixEdit implements bridgev2.EditHandlingNetworkAPI. Like a real network, the edit is
// echoed back as a remote event from the local user.
func (c *Client) HandleMatrixEdit(_ context.Context, msg *bridgev2.MatrixEdit) error {
	text := msg.Content.Body
	if msg.Content.NewContent != nil {
		text = msg.Content.NewContent.Body
	}
	c.queue(c.login.Log.WithContext(c.login.Bridge.BackgroundCtx), &simplevent.Message[string]{
		EventMeta:     simplevent.EventMeta{Type: bridgev2.RemoteEventEdit, PortalKey: msg.Portal.PortalKey, Sender: bridgev2.EventSender{IsFromMe: true}, Timestamp: time.Now()},
		Data:          text,
		TargetMessage: msg.EditTarget.ID,
		ConvertEditFunc: func(_ context.Context, _ *bridgev2.Portal, _ bridgev2.MatrixAPI, existing []*database.Message, text string) (*bridgev2.ConvertedEdit, error) {
			return &bridgev2.ConvertedEdit{ModifiedParts: []*bridgev2.ConvertedEditPart{{
				Part:    existing[0],
				Type:    event.EventMessage,
				Content: &event.MessageEventContent{MsgType: event.MsgText, Body: text},
			}}}, nil
		},
	})
	return nil
}

// HandleMatrixMessageRemove implements bridgev2.RedactionHandlingNetworkAPI. The removal is
// accepted without an echo: bridgev2 forgets a message once its Matrix redaction is handled, so
// a remote removal echo would find nothing to remove.
func (c *Client) HandleMatrixMessageRemove(context.Context, *bridgev2.MatrixMessageRemove) error {
	return nil
}

// HandleMatrixReadReceipt implements bridgev2.ReadReceiptHandlingNetworkAPI.
func (c *Client) HandleMatrixReadReceipt(context.Context, *bridgev2.MatrixReadReceipt) error {
	return nil
}

// HandleMatrixTyping implements bridgev2.TypingHandlingNetworkAPI.
func (c *Client) HandleMatrixTyping(context.Context, *bridgev2.MatrixTyping) error {
	return nil
}

// FetchMessages implements bridgev2.BackfillingNetworkAPI. Backward pages hold 20 synthetic
// messages each, 3 pages in total. Every page is the same on every call: the message IDs are
// fixed, so bridgev2 deduplicates a repeated fetch, and all history is older than the seeded
// messages. Forward fetches have nothing new.
func (c *Client) FetchMessages(_ context.Context, params bridgev2.FetchMessagesParams) (*bridgev2.FetchMessagesResponse, error) {
	if params.Forward {
		return &bridgev2.FetchMessagesResponse{Forward: true}, nil
	}
	page := 0
	if params.Cursor != "" {
		if _, err := fmt.Sscanf(string(params.Cursor), "page-%d", &page); err != nil {
			return nil, fmt.Errorf("invalid cursor %q: %w", params.Cursor, err)
		}
	}
	key := params.Portal.PortalKey
	msgs := make([]*bridgev2.BackfillMessage, 0, historyPageSize)
	// Oldest first: history message n is n minutes before the first seeded message.
	for i := historyPageSize - 1; i >= 0; i-- {
		n := page*historyPageSize + i + 1
		sender := bridgev2.EventSender{Sender: "alice"}
		if i%2 == 1 {
			sender = bridgev2.EventSender{Sender: "bob"}
		}
		msgs = append(msgs, &bridgev2.BackfillMessage{
			ConvertedMessage: textConverted(historyText(n)),
			Sender:           sender,
			ID:               networkid.MessageID(fmt.Sprintf("hist-%s-%d", key.ID, n)),
			Timestamp:        seedStart.Add(-time.Duration(n) * time.Minute),
		})
	}
	return &bridgev2.FetchMessagesResponse{
		Messages: msgs,
		Cursor:   networkid.PaginationCursor(fmt.Sprintf("page-%d", page+1)),
		HasMore:  page+1 < historyPages,
	}, nil
}

// seed queues the fixed chats and their messages, oldest first.
func (c *Client) seed(ctx context.Context) {
	// Reactions by the local user reference its own ghost row, so the bridge needs it to exist.
	if _, err := c.login.Bridge.GetGhostByID(ctx, selfID); err != nil {
		zerolog.Ctx(ctx).Err(err).Msg("Failed to create the demo self ghost")
		return
	}
	base := seedStart
	for _, chat := range seedChats {
		var last time.Time
		events := make([]bridgev2.RemoteEvent, 0, len(chat.messages))
		for i, m := range chat.messages {
			ts := base.Add(time.Duration(i) * time.Minute)
			last = ts
			id := networkid.MessageID(fmt.Sprintf("%s-%d", chat.key.ID, i))
			events = append(events, newTextMessage(chat.key, senderFor(m.sender), id, m.text, ts))
		}
		c.queue(ctx, &simplevent.ChatResync{
			EventMeta:       simplevent.EventMeta{Type: bridgev2.RemoteEventChatResync, PortalKey: chat.key, CreatePortal: true, Timestamp: base},
			ChatInfo:        chatInfo(chat),
			LatestMessageTS: last,
		})
		if err := c.awaitRoom(ctx, chat.key); err != nil {
			zerolog.Ctx(ctx).Err(err).Msg("Demo chat was not created")
			return
		}
		for _, evt := range events {
			c.queue(ctx, evt)
		}
	}
}

// awaitRoom waits until the portal of key has its Matrix room saved in the bridge database.
// bridgev2 reads a portal's room ID without locking when queueing events for it, so events are
// queued only once the room has been saved, which orders the writes before the reads.
func (c *Client) awaitRoom(ctx context.Context, key networkid.PortalKey) error {
	deadline := time.Now().Add(20 * time.Second)
	for {
		portal, err := c.login.Bridge.DB.Portal.GetByKey(ctx, key)
		if err != nil {
			return fmt.Errorf("failed to load portal %s: %w", key.ID, err)
		}
		if portal != nil && portal.MXID != "" {
			return nil
		}
		if time.Now().After(deadline) {
			return fmt.Errorf("portal %s has no room after 20s", key.ID)
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(10 * time.Millisecond):
		}
	}
}

// queue hands a remote event to the bridge and logs rejected events.
func (c *Client) queue(ctx context.Context, evt bridgev2.RemoteEvent) {
	if res := c.login.QueueRemoteEvent(evt); !res.Success {
		zerolog.Ctx(ctx).Warn().Stringer("event_type", evt.GetType()).AnErr("error", res.Error).
			Msg("Demo network event was not accepted")
	}
}

// textConverted is a converted message with one plain text part.
func textConverted(text string) *bridgev2.ConvertedMessage {
	return &bridgev2.ConvertedMessage{Parts: []*bridgev2.ConvertedMessagePart{{
		Type:    event.EventMessage,
		Content: &event.MessageEventContent{MsgType: event.MsgText, Body: text},
	}}}
}

// newTextMessage is a remote text message that can be queued on the login.
func newTextMessage(key networkid.PortalKey, sender bridgev2.EventSender, id networkid.MessageID, text string, ts time.Time) *simplevent.Message[string] {
	return &simplevent.Message[string]{
		EventMeta: simplevent.EventMeta{Type: bridgev2.RemoteEventMessage, PortalKey: key, Sender: sender, Timestamp: ts},
		Data:      text,
		ID:        id,
		ConvertMessageFunc: func(ctx context.Context, portal *bridgev2.Portal, intent bridgev2.MatrixAPI, text string) (*bridgev2.ConvertedMessage, error) {
			return textConverted(text), nil
		},
	}
}
