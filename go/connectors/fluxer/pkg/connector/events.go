// SPDX-License-Identifier: AGPL-3.0-or-later

package connector

import (
	"context"
	"encoding/json"
	"time"

	"github.com/rs/zerolog"
	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/database"
	"maunium.net/go/mautrix/bridgev2/networkid"
	"maunium.net/go/mautrix/bridgev2/simplevent"
	"maunium.net/go/mautrix/bridgev2/status"
	"maunium.net/go/mautrix/id"

	"chirper.dev/connectors/fluxer/pkg/fluxerapi"
)

// typingTimeout is how long a typing indicator stays visible without a repeat.
const typingTimeout = 10 * time.Second

// handleEvent converts one Gateway Dispatch into bridgev2 remote events. Payloads are decoded
// into typed structs and never logged; only event names are logged on failure.
func (c *fluxerClient) handleEvent(ctx context.Context, ev fluxerapi.Event) {
	var err error
	switch ev.Type {
	case "READY":
		err = c.onReady(ctx, ev.Data)
	case "GUILD_CREATE":
		err = c.onGuildCreate(ctx, ev.Data)
	case "GUILD_DELETE":
		err = c.onGuildDelete(ev.Data)
	case "CHANNEL_CREATE", "CHANNEL_UPDATE":
		err = c.onChannel(ev.Data)
	case "CHANNEL_DELETE":
		err = c.onChannelDelete(ev.Data)
	case "MESSAGE_CREATE":
		err = c.onMessage(ev.Data, bridgev2.RemoteEventMessage)
	case "MESSAGE_UPDATE":
		err = c.onMessage(ev.Data, bridgev2.RemoteEventEdit)
	case "MESSAGE_DELETE":
		err = c.onMessageDelete(ev.Data)
	case "MESSAGE_DELETE_BULK":
		err = c.onMessageDeleteBulk(ev.Data)
	case "MESSAGE_REACTION_ADD":
		err = c.onReaction(ev.Data, bridgev2.RemoteEventReaction)
	case "MESSAGE_REACTION_REMOVE":
		err = c.onReaction(ev.Data, bridgev2.RemoteEventReactionRemove)
	case "TYPING_START":
		err = c.onTyping(ev.Data)
	case "MESSAGE_ACK":
		err = c.onAck(ev.Data)
	}
	if err != nil {
		zerolog.Ctx(ctx).Warn().Str("event", ev.Type).Err(err).Msg("Ignoring malformed gateway event")
	}
}

func (c *fluxerClient) onReady(ctx context.Context, data json.RawMessage) error {
	var ready fluxerapi.Ready
	if err := json.Unmarshal(data, &ready); err != nil {
		return err
	}
	if c.login != nil {
		changed := false
		if ready.User.Avatar != nil && *ready.User.Avatar != "" {
			mediaBase := c.meta.MediaBase
			if mediaBase == "" {
				mediaBase = c.meta.APIBase
			}
			newAvatar := id.ContentURIString(imageURL(mediaBase, "/avatars/"+ready.User.ID, *ready.User.Avatar))
			if c.login.RemoteProfile.Avatar != newAvatar {
				c.login.RemoteProfile.Avatar = newAvatar
				changed = true
			}
		}
		displayName := ready.User.DisplayName()
		if displayName != "" && c.login.RemoteProfile.Name != displayName {
			c.login.RemoteProfile.Name = displayName
			changed = true
		}
		if ready.User.Username != "" && c.login.RemoteProfile.Username != ready.User.Username {
			c.login.RemoteProfile.Username = ready.User.Username
			changed = true
		}
		if changed {
			_ = c.login.Save(ctx)
			c.login.BridgeState.Send(status.BridgeState{StateEvent: status.StateConnected})
		}
	}
	for _, ch := range ready.PrivateChannels {
		c.rememberChannel(ch.ID, "")
		c.queueChat(ch, "")
	}
	guildIDs := make([]string, 0, len(ready.Guilds))
	for _, g := range ready.Guilds {
		if g.Unavailable {
			continue
		}
		guildIDs = append(guildIDs, g.ID)
		c.queueGuild(g)
	}
	c.subscribe(ctx, guildIDs)
	return nil
}

func (c *fluxerClient) onGuildCreate(ctx context.Context, data json.RawMessage) error {
	var g fluxerapi.GuildState
	if err := json.Unmarshal(data, &g); err != nil {
		return err
	}
	if g.Unavailable {
		return nil
	}
	c.queueGuild(g)
	c.subscribe(ctx, []string{g.ID})
	return nil
}

// queueGuild bridges a guild as a space and its message channels as children of that space.
func (c *fluxerClient) queueGuild(g fluxerapi.GuildState) {
	c.rememberSpace(g.ID)
	c.queueSpace(g.Properties)
	for _, ch := range g.Channels {
		c.rememberChannel(ch.ID, g.ID)
		if ch.IsMessageChannel() {
			c.queueChat(ch, g.ID)
		}
	}
}

func (c *fluxerClient) onGuildDelete(data json.RawMessage) error {
	var ref fluxerapi.GuildRef
	if err := json.Unmarshal(data, &ref); err != nil {
		return err
	}
	if ref.Unavailable {
		return nil
	}
	c.login.QueueRemoteEvent(&simplevent.ChatDelete{
		EventMeta: simplevent.EventMeta{
			Type:      bridgev2.RemoteEventChatDelete,
			PortalKey: makePortalKey(ref.ID, false, c.login.ID),
		},
		Children: true,
	})
	return nil
}

func (c *fluxerClient) onChannel(data json.RawMessage) error {
	var ch fluxerapi.Channel
	if err := json.Unmarshal(data, &ch); err != nil {
		return err
	}
	c.rememberChannel(ch.ID, ch.GuildID)
	if ch.IsMessageChannel() {
		c.queueChat(ch, ch.GuildID)
	}
	return nil
}

func (c *fluxerClient) onChannelDelete(data json.RawMessage) error {
	var ch fluxerapi.Channel
	if err := json.Unmarshal(data, &ch); err != nil {
		return err
	}
	c.login.QueueRemoteEvent(&simplevent.ChatDelete{
		EventMeta: simplevent.EventMeta{
			Type:      bridgev2.RemoteEventChatDelete,
			PortalKey: c.portalKeyFor(ch.ID, ch.GuildID),
		},
	})
	return nil
}

// onMessage handles MESSAGE_CREATE (as a new message) and MESSAGE_UPDATE (as an edit).
func (c *fluxerClient) onMessage(data json.RawMessage, typ bridgev2.RemoteEventType) error {
	var msg fluxerapi.Message
	if err := json.Unmarshal(data, &msg); err != nil {
		return err
	}
	evt := &simplevent.Message[*fluxerapi.Message]{
		EventMeta: simplevent.EventMeta{
			Type:         typ,
			PortalKey:    c.portalKeyFor(msg.ChannelID, msg.GuildID),
			Sender:       c.senderOf(msg.Author.ID),
			CreatePortal: typ == bridgev2.RemoteEventMessage,
			Timestamp:    msg.Timestamp,
			StreamOrder:  msg.Timestamp.UnixMilli(),
		},
		Data:               &msg,
		ID:                 makeMessageID(msg.ID),
		TargetMessage:      makeMessageID(msg.ID),
		TransactionID:      networkid.TransactionID(msg.NonceString()),
		ConvertMessageFunc: c.convertMessage,
		ConvertEditFunc:    c.convertEdit,
		HandleExistingFunc: handleExisting,
	}
	c.login.QueueRemoteEvent(evt)
	return nil
}

func (c *fluxerClient) onMessageDelete(data json.RawMessage) error {
	var del fluxerapi.MessageDeleteEvent
	if err := json.Unmarshal(data, &del); err != nil {
		return err
	}
	c.queueRemove(del.ChannelID, del.GuildID, del.ID)
	return nil
}

func (c *fluxerClient) onMessageDeleteBulk(data json.RawMessage) error {
	var bulk fluxerapi.MessageDeleteBulkEvent
	if err := json.Unmarshal(data, &bulk); err != nil {
		return err
	}
	for _, id := range bulk.IDs {
		c.queueRemove(bulk.ChannelID, bulk.GuildID, id)
	}
	return nil
}

func (c *fluxerClient) queueRemove(channelID, guildID, messageID string) {
	c.login.QueueRemoteEvent(&simplevent.MessageRemove{
		EventMeta: simplevent.EventMeta{
			Type:      bridgev2.RemoteEventMessageRemove,
			PortalKey: c.portalKeyFor(channelID, guildID),
		},
		TargetMessage: makeMessageID(messageID),
	})
}

func (c *fluxerClient) onReaction(data json.RawMessage, typ bridgev2.RemoteEventType) error {
	var r fluxerapi.ReactionEvent
	if err := json.Unmarshal(data, &r); err != nil {
		return err
	}
	now := time.Now()
	c.login.QueueRemoteEvent(&simplevent.Reaction{
		EventMeta: simplevent.EventMeta{
			Type:        typ,
			PortalKey:   c.portalKeyFor(r.ChannelID, r.GuildID),
			Sender:      c.senderOf(r.UserID),
			Timestamp:   now,
			StreamOrder: now.UnixMilli(),
		},
		TargetMessage: makeMessageID(r.MessageID),
		EmojiID:       emojiID(r.Emoji),
		Emoji:         emojiText(r.Emoji),
	})
	return nil
}

func (c *fluxerClient) onTyping(data json.RawMessage) error {
	var t fluxerapi.TypingEvent
	if err := json.Unmarshal(data, &t); err != nil {
		return err
	}
	if t.UserID == c.myID() {
		return nil
	}
	c.login.QueueRemoteEvent(&simplevent.Typing{
		EventMeta: simplevent.EventMeta{
			Type:      bridgev2.RemoteEventTyping,
			PortalKey: c.portalKeyFor(t.ChannelID, t.GuildID),
			Sender:    c.senderOf(t.UserID),
		},
		Timeout: typingTimeout,
		Type:    bridgev2.TypingTypeText,
	})
	return nil
}

// onAck mirrors read state from the account's other sessions.
func (c *fluxerClient) onAck(data json.RawMessage) error {
	var ack fluxerapi.MessageAckEvent
	if err := json.Unmarshal(data, &ack); err != nil {
		return err
	}
	c.login.QueueRemoteEvent(&simplevent.Receipt{
		EventMeta: simplevent.EventMeta{
			Type:      bridgev2.RemoteEventReadReceipt,
			PortalKey: c.portalKeyFor(ack.ChannelID, ""),
			Sender:    c.senderOf(c.myID()),
		},
		LastTarget: makeMessageID(ack.MessageID),
	})
	return nil
}

// queueChat sends a ChatResync that creates or updates a portal. parentGuild is the guild ID for
// guild channels, or "" for direct chats.
func (c *fluxerClient) queueChat(ch fluxerapi.Channel, parentGuild string) {
	c.login.QueueRemoteEvent(&simplevent.ChatResync{
		EventMeta: simplevent.EventMeta{
			Type:         bridgev2.RemoteEventChatResync,
			PortalKey:    c.portalKeyFor(ch.ID, parentGuild),
			CreatePortal: true,
		},
		ChatInfo: c.channelChatInfo(ch, parentGuild),
	})
}

func (c *fluxerClient) queueSpace(g fluxerapi.Guild) {
	c.login.QueueRemoteEvent(&simplevent.ChatResync{
		EventMeta: simplevent.EventMeta{
			Type:         bridgev2.RemoteEventChatResync,
			PortalKey:    makePortalKey(g.ID, false, c.login.ID),
			CreatePortal: true,
		},
		ChatInfo: c.guildChatInfo(g),
	})
}

// subscribe makes the given guilds active for user sessions. Bots are never passive, so they skip it.
func (c *fluxerClient) subscribe(ctx context.Context, guildIDs []string) {
	if c.cred.Bot || len(guildIDs) == 0 {
		return
	}
	c.mu.Lock()
	gw := c.gw
	c.mu.Unlock()
	if gw == nil {
		return
	}
	if err := gw.SubscribeGuilds(ctx, guildIDs); err != nil {
		zerolog.Ctx(ctx).Warn().Err(err).Msg("Failed to subscribe to guild events")
	}
}

// handleExisting runs when a create event repeats a stored message. Nothing needs to change.
func handleExisting(_ context.Context, _ *bridgev2.Portal, _ bridgev2.MatrixAPI, _ []*database.Message, _ *fluxerapi.Message) (bridgev2.UpsertResult, error) {
	return bridgev2.UpsertResult{}, nil
}

func emojiID(e fluxerapi.Emoji) networkid.EmojiID {
	if e.ID != nil && *e.ID != "" {
		return networkid.EmojiID(*e.ID)
	}
	return networkid.EmojiID(e.Name)
}

func emojiText(e fluxerapi.Emoji) string {
	if e.ID != nil && *e.ID != "" {
		return ":" + e.Name + ":"
	}
	return e.Name
}
