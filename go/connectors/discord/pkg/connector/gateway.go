// Chirper - Discord bridgev2 network connector.
// Copyright (C) 2026 Chirper contributors
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

package connector

import (
	"time"

	"github.com/bwmarrin/discordgo"
	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/networkid"
	"maunium.net/go/mautrix/bridgev2/simplevent"
	"maunium.net/go/mautrix/bridgev2/status"
)

// typingTimeout is how long Discord shows a typing indicator after a TYPING_START event.
const typingTimeout = 10 * time.Second

// handleEvent converts one gateway event into bridgev2 remote events.
func (c *DiscordClient) handleEvent(raw any) {
	switch evt := raw.(type) {
	case *discordgo.Ready:
		c.onReady(evt)
	case *discordgo.Resumed:
		c.sendState(status.BridgeState{StateEvent: status.StateConnected})
	case *discordgo.Disconnect:
		c.sendState(status.BridgeState{StateEvent: status.StateTransientDisconnect, Error: "discord-gateway-disconnected"})
	case *discordgo.InvalidAuth:
		c.sendState(status.BridgeState{StateEvent: status.StateBadCredentials, Error: "discord-invalid-token"})
	case *discordgo.GuildCreate:
		c.onGuildCreate(evt.Guild)
	case *discordgo.GuildUpdate:
		c.queueSpaceResync(evt.Guild)
	case *discordgo.GuildDelete:
		c.queueChatDelete(makeGuildPortalKey(evt.ID), false)
	case *discordgo.ChannelCreate:
		c.queueChannelResync(evt.Channel)
	case *discordgo.ChannelUpdate:
		c.queueChannelResync(evt.Channel)
	case *discordgo.ThreadCreate:
		c.queueChannelResync(evt.Channel)
	case *discordgo.ChannelDelete:
		c.queueChatDelete(makePortalKey(evt.ID), false)
	case *discordgo.MessageCreate:
		c.onMessage(evt.Message, false)
	case *discordgo.MessageUpdate:
		c.onMessage(evt.Message, true)
	case *discordgo.MessageDelete:
		c.queueMessageRemove(evt.ChannelID, evt.ID)
	case *discordgo.MessageReactionAdd:
		c.queueReaction(evt.MessageReaction, true)
	case *discordgo.MessageReactionRemove:
		c.queueReaction(evt.MessageReaction, false)
	case *discordgo.TypingStart:
		c.onTyping(evt)
	case *discordgo.MessageAck:
		c.onMessageAck(evt)
	}
}

func (c *DiscordClient) queue(evt bridgev2.RemoteEvent) {
	c.UserLogin.QueueRemoteEvent(evt)
}

// makeSender builds the bridgev2 sender for a Discord user ID.
func (c *DiscordClient) makeSender(userID string) bridgev2.EventSender {
	return bridgev2.EventSender{
		IsFromMe:    userID == c.selfUserID(),
		SenderLogin: makeUserLoginID(userID),
		Sender:      makeUserID(userID),
	}
}

func eventMeta(portal networkid.PortalKey, sender bridgev2.EventSender, ts time.Time, createPortal bool) simplevent.EventMeta {
	meta := simplevent.EventMeta{
		PortalKey:    portal,
		Sender:       sender,
		Timestamp:    ts,
		CreatePortal: createPortal,
	}
	if !ts.IsZero() {
		meta.StreamOrder = ts.UnixMicro()
	}
	return meta
}

func (c *DiscordClient) onReady(evt *discordgo.Ready) {
	c.setGatewayIdentity(evt.User.ID, evt.SessionID)
	c.sendState(status.BridgeState{StateEvent: status.StateConnected})
	for _, dm := range evt.PrivateChannels {
		c.queueChannelResync(dm)
	}
}

func (c *DiscordClient) onGuildCreate(guild *discordgo.Guild) {
	c.queueSpaceResync(guild)
	for _, ch := range guild.Channels {
		if !c.reserveGuildChannelSync(ch) {
			return
		}
		c.queueChannelResync(ch)
	}
}

// reserveGuildChannelSync reports whether an eager channel portal may be created. Only the first
// SyncChannelLimit guild channels are created up front; the rest are created lazily on first message.
func (c *DiscordClient) reserveGuildChannelSync(ch *discordgo.Channel) bool {
	if !isBridgeableGuildChannel(ch) {
		return true
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.guildChannelsSynced >= c.Main.Config.SyncChannelLimit {
		return false
	}
	c.guildChannelsSynced++
	return true
}

func isBridgeableGuildChannel(ch *discordgo.Channel) bool {
	return ch.Type == discordgo.ChannelTypeGuildText || ch.Type == discordgo.ChannelTypeGuildNews
}

func (c *DiscordClient) queueSpaceResync(guild *discordgo.Guild) {
	meta := eventMeta(makeGuildPortalKey(guild.ID), bridgev2.EventSender{}, time.Time{}, true)
	c.queue(&simplevent.ChatResync{
		EventMeta: meta.WithType(bridgev2.RemoteEventChatResync),
		ChatInfo:  c.wrapGuildInfo(guild),
	})
}

func (c *DiscordClient) queueChannelResync(ch *discordgo.Channel) {
	meta := eventMeta(makePortalKey(ch.ID), bridgev2.EventSender{}, time.Time{}, true)
	c.queue(&simplevent.ChatResync{
		EventMeta: meta.WithType(bridgev2.RemoteEventChatResync),
		ChatInfo:  c.wrapChannelInfo(ch),
	})
}

func (c *DiscordClient) queueChatDelete(portal networkid.PortalKey, createPortal bool) {
	meta := eventMeta(portal, bridgev2.EventSender{}, time.Time{}, createPortal)
	c.queue(&simplevent.ChatDelete{EventMeta: meta.WithType(bridgev2.RemoteEventChatDelete)})
}

func (c *DiscordClient) queueMessageRemove(channelID, messageID string) {
	meta := eventMeta(makePortalKey(channelID), bridgev2.EventSender{}, time.Time{}, false)
	c.queue(&simplevent.MessageRemove{
		EventMeta:     meta.WithType(bridgev2.RemoteEventMessageRemove),
		TargetMessage: makeMessageID(messageID),
	})
}

func (c *DiscordClient) onMessage(msg *discordgo.Message, isEdit bool) {
	if msg.Author == nil {
		// MESSAGE_UPDATE can be partial (for example, embeds resolving). Fetch the full message.
		sess, err := c.currentSession()
		if err != nil {
			return
		}
		full, err := sess.ChannelMessage(msg.ChannelID, msg.ID)
		if err != nil || full.Author == nil {
			c.UserLogin.Log.Debug().Err(err).Str("message_id", msg.ID).Msg("Skipping partial message update")
			return
		}
		msg = full
	}
	if msg.Author == nil {
		return
	}
	c.queue(c.wrapMessage(msg, isEdit))
}

func (c *DiscordClient) wrapMessage(msg *discordgo.Message, isEdit bool) bridgev2.RemoteEvent {
	ts := msg.Timestamp
	evtType := bridgev2.RemoteEventMessage
	if isEdit {
		evtType = bridgev2.RemoteEventEdit
		if msg.EditedTimestamp != nil {
			ts = *msg.EditedTimestamp
		}
	}
	meta := eventMeta(makePortalKey(msg.ChannelID), c.makeSender(msg.Author.ID), ts, true)
	evt := &simplevent.Message[*discordgo.Message]{
		EventMeta:          meta.WithType(evtType),
		Data:               msg,
		ID:                 makeMessageID(msg.ID),
		TransactionID:      networkid.TransactionID(msg.Nonce),
		ConvertMessageFunc: c.convertMessage,
		ConvertEditFunc:    c.convertEdit,
	}
	if isEdit {
		evt.TargetMessage = evt.ID
	}
	return evt
}

func (c *DiscordClient) queueReaction(r *discordgo.MessageReaction, add bool) {
	emoji, emojiID := reactionEmoji(r.Emoji)
	meta := eventMeta(makePortalKey(r.ChannelID), c.makeSender(r.UserID), time.Time{}, false)
	if add {
		c.queue(&simplevent.Reaction{
			EventMeta:     meta.WithType(bridgev2.RemoteEventReaction),
			TargetMessage: makeMessageID(r.MessageID),
			EmojiID:       emojiID,
			Emoji:         emoji,
		})
		return
	}
	c.queue(&simplevent.Reaction{
		EventMeta:     meta.WithType(bridgev2.RemoteEventReactionRemove),
		TargetMessage: makeMessageID(r.MessageID),
		EmojiID:       emojiID,
		Emoji:         emoji,
	})
}

// reactionEmoji returns the Matrix-facing key and the stable bridge ID of a Discord emoji.
// Custom emoji use ":name:" as the key, so the same reaction can be sent back by name.
func reactionEmoji(e discordgo.Emoji) (string, networkid.EmojiID) {
	if e.ID == "" {
		return e.Name, networkid.EmojiID(e.Name)
	}
	return ":" + e.Name + ":", networkid.EmojiID("custom:" + e.ID)
}

func (c *DiscordClient) onTyping(evt *discordgo.TypingStart) {
	if evt.UserID == c.selfUserID() {
		return
	}
	meta := eventMeta(makePortalKey(evt.ChannelID), c.makeSender(evt.UserID), time.Time{}, false)
	c.queue(&simplevent.Typing{
		EventMeta: meta.WithType(bridgev2.RemoteEventTyping),
		Timeout:   typingTimeout,
		Type:      bridgev2.TypingTypeText,
	})
}

// onMessageAck mirrors read state from the user's other Discord clients. Bot accounts have no read state.
func (c *DiscordClient) onMessageAck(evt *discordgo.MessageAck) {
	if c.isBot() {
		return
	}
	meta := eventMeta(makePortalKey(evt.ChannelID), c.makeSender(c.selfUserID()), time.Time{}, false)
	c.queue(&simplevent.Receipt{
		EventMeta:  meta.WithType(bridgev2.RemoteEventReadReceipt),
		LastTarget: makeMessageID(evt.MessageID),
		Targets:    []networkid.MessageID{makeMessageID(evt.MessageID)},
		ReadUpTo:   time.Now(),
	})
}
