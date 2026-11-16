// Chirper - Discord bridgev2 network connector.
// Copyright (C) 2026 Chirper contributors
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

package connector

import (
	"bytes"
	"context"
	"fmt"
	"net/url"
	"regexp"
	"strings"

	"github.com/bwmarrin/discordgo"
	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/database"
	"maunium.net/go/mautrix/bridgev2/networkid"
	"maunium.net/go/mautrix/event"
)

// discordNonceMaxLength is Discord's limit for the nonce used to match our own echoed messages.
const discordNonceMaxLength = 25

var customEmojiKeyPattern = regexp.MustCompile(`^<a?:(\w+):(\d+)>$`)

// HandleMatrixMessage sends a message, file or sticker from Matrix to Discord.
func (c *DiscordClient) HandleMatrixMessage(ctx context.Context, msg *bridgev2.MatrixMessage) (*bridgev2.MatrixMessageResponse, error) {
	sess, err := c.currentSession()
	if err != nil {
		return nil, err
	}
	channelID := string(msg.Portal.ID)
	send := &discordgo.MessageSend{}
	var mentioned []string
	if isMediaMessage(msg) {
		send.Content = mediaCaption(msg.Content)
		send.Files, err = c.matrixFile(ctx, msg)
		if err != nil {
			return nil, err
		}
	} else {
		send.Content, mentioned = matrixHTMLToDiscord(c.Main.br.Matrix, msg.Content)
	}
	send.AllowedMentions = allowedMentions(mentioned)
	if msg.ReplyTo != nil {
		send.Reference = &discordgo.MessageReference{MessageID: string(msg.ReplyTo.ID), ChannelID: channelID}
	}
	if txn := string(msg.InputTransactionID); len(txn) > 0 && len(txn) <= discordNonceMaxLength {
		send.Nonce = txn
	}
	sent, err := sess.ChannelMessageSendComplex(channelID, send)
	if err != nil {
		return nil, fmt.Errorf("failed to send message to Discord: %w", err)
	}
	return &bridgev2.MatrixMessageResponse{
		DB: &database.Message{
			ID:        makeMessageID(sent.ID),
			SenderID:  makeUserID(c.selfUserID()),
			Timestamp: sent.Timestamp,
		},
		StreamOrder: sent.Timestamp.UnixMicro(),
	}, nil
}

// isMediaMessage reports whether the Matrix event carries a file. Stickers are sent as images.
func isMediaMessage(msg *bridgev2.MatrixMessage) bool {
	return msg.Event.Type == event.EventSticker || msg.Content.MsgType.IsMedia()
}

// mediaCaption returns the caption of a media message. Matrix uses the body as the file name when
// no caption is set, so the body only becomes a caption when it differs from the file name.
func mediaCaption(content *event.MessageEventContent) string {
	if content.FileName != "" && content.Body == content.FileName {
		return ""
	}
	if content.Body == "" {
		return ""
	}
	return content.Body
}

func (c *DiscordClient) matrixFile(ctx context.Context, msg *bridgev2.MatrixMessage) ([]*discordgo.File, error) {
	content := msg.Content
	data, err := msg.Portal.Bridge.Bot.DownloadMedia(ctx, content.URL, content.File)
	if err != nil {
		return nil, fmt.Errorf("failed to download media from Matrix: %w", err)
	}
	name := content.FileName
	if name == "" {
		name = content.Body
	}
	mimeType := "application/octet-stream"
	if content.Info != nil && content.Info.MimeType != "" {
		mimeType = content.Info.MimeType
	}
	return []*discordgo.File{{Name: name, ContentType: mimeType, Reader: bytes.NewReader(data)}}, nil
}

func allowedMentions(userIDs []string) *discordgo.MessageAllowedMentions {
	if userIDs == nil {
		userIDs = []string{}
	}
	return &discordgo.MessageAllowedMentions{
		Parse:       []discordgo.AllowedMentionType{},
		Users:       userIDs,
		RepliedUser: true,
	}
}

// HandleMatrixEdit edits the text of a previously sent Discord message.
func (c *DiscordClient) HandleMatrixEdit(ctx context.Context, msg *bridgev2.MatrixEdit) error {
	sess, err := c.currentSession()
	if err != nil {
		return err
	}
	text, mentioned := matrixHTMLToDiscord(c.Main.br.Matrix, msg.Content)
	_, err = sess.ChannelMessageEditComplex(&discordgo.MessageEdit{
		ID:              string(msg.EditTarget.ID),
		Channel:         string(msg.Portal.ID),
		Content:         &text,
		AllowedMentions: allowedMentions(mentioned),
	})
	if err != nil {
		return fmt.Errorf("failed to edit message on Discord: %w", err)
	}
	return nil
}

// HandleMatrixMessageRemove deletes a Discord message.
func (c *DiscordClient) HandleMatrixMessageRemove(ctx context.Context, msg *bridgev2.MatrixMessageRemove) error {
	sess, err := c.currentSession()
	if err != nil {
		return err
	}
	if err = sess.ChannelMessageDelete(string(msg.Portal.ID), string(msg.TargetMessage.ID)); err != nil {
		return fmt.Errorf("failed to delete message on Discord: %w", err)
	}
	return nil
}

// PreHandleMatrixReaction resolves a Matrix reaction key to a Discord emoji before the reaction is sent.
// Keys can be unicode emoji, ":name:" for a custom emoji of the guild, or "<:name:id>" as Discord writes them.
func (c *DiscordClient) PreHandleMatrixReaction(ctx context.Context, msg *bridgev2.MatrixReaction) (bridgev2.MatrixReactionPreResponse, error) {
	sess, err := c.currentSession()
	if err != nil {
		return bridgev2.MatrixReactionPreResponse{}, err
	}
	key := msg.Content.RelatesTo.Key
	resp := bridgev2.MatrixReactionPreResponse{SenderID: makeUserID(c.selfUserID())}
	switch {
	case customEmojiKeyPattern.MatchString(key):
		match := customEmojiKeyPattern.FindStringSubmatch(key)
		resp.Emoji = ":" + match[1] + ":"
		resp.EmojiID = networkid.EmojiID("custom:" + match[2])
	case strings.HasPrefix(key, ":") && strings.HasSuffix(key, ":") && len(key) > 2:
		emojiID, err := c.findGuildEmoji(sess, string(msg.Portal.ID), strings.Trim(key, ":"))
		if err != nil {
			return resp, err
		}
		resp.Emoji = key
		resp.EmojiID = networkid.EmojiID("custom:" + emojiID)
	default:
		resp.Emoji = key
		resp.EmojiID = networkid.EmojiID(key)
	}
	return resp, nil
}

// findGuildEmoji returns the ID of a custom emoji by name in the guild that contains the channel.
func (c *DiscordClient) findGuildEmoji(sess *discordgo.Session, channelID, name string) (string, error) {
	ch, err := c.getChannel(sess, channelID)
	if err != nil {
		return "", err
	}
	guild, err := sess.State.Guild(ch.GuildID)
	if err != nil {
		return "", fmt.Errorf("unknown custom emoji :%s: (not in a guild)", name)
	}
	for _, emoji := range guild.Emojis {
		if emoji.Name == name {
			return emoji.ID, nil
		}
	}
	return "", fmt.Errorf("unknown custom emoji :%s:", name)
}

// discordReactionKey converts the bridge emoji ID back into the key Discord's REST API expects.
func discordReactionKey(emojiID networkid.EmojiID, emoji string) string {
	if customID, ok := strings.CutPrefix(string(emojiID), "custom:"); ok {
		return strings.Trim(emoji, ":") + ":" + customID
	}
	return string(emojiID)
}

// HandleMatrixReaction adds a reaction to a Discord message.
func (c *DiscordClient) HandleMatrixReaction(ctx context.Context, msg *bridgev2.MatrixReaction) (*database.Reaction, error) {
	sess, err := c.currentSession()
	if err != nil {
		return nil, err
	}
	key := discordReactionKey(msg.PreHandleResp.EmojiID, msg.PreHandleResp.Emoji)
	err = sess.MessageReactionAdd(string(msg.Portal.ID), string(msg.TargetMessage.ID), url.PathEscape(key))
	if err != nil {
		return nil, fmt.Errorf("failed to add reaction on Discord: %w", err)
	}
	return nil, nil
}

// HandleMatrixReactionRemove removes the bridged user's reaction from a Discord message.
func (c *DiscordClient) HandleMatrixReactionRemove(ctx context.Context, msg *bridgev2.MatrixReactionRemove) error {
	sess, err := c.currentSession()
	if err != nil {
		return err
	}
	key := discordReactionKey(msg.TargetReaction.EmojiID, msg.TargetReaction.Emoji)
	err = sess.MessageReactionRemove(string(msg.Portal.ID), string(msg.TargetReaction.MessageID), url.PathEscape(key), "@me")
	if err != nil {
		return fmt.Errorf("failed to remove reaction on Discord: %w", err)
	}
	return nil
}

// HandleMatrixReadReceipt acknowledges a message on Discord. Bot accounts have no read state.
func (c *DiscordClient) HandleMatrixReadReceipt(ctx context.Context, msg *bridgev2.MatrixReadReceipt) error {
	if c.isBot() || msg.ExactMessage == nil {
		return nil
	}
	sess, err := c.currentSession()
	if err != nil {
		return err
	}
	if _, err = sess.ChannelMessageAck(string(msg.Portal.ID), string(msg.ExactMessage.ID), ""); err != nil {
		return fmt.Errorf("failed to acknowledge message on Discord: %w", err)
	}
	return nil
}

// HandleMatrixTyping sends a typing indicator. Discord only has one typing state, so stop is a no-op.
func (c *DiscordClient) HandleMatrixTyping(ctx context.Context, msg *bridgev2.MatrixTyping) error {
	if !msg.IsTyping {
		return nil
	}
	sess, err := c.currentSession()
	if err != nil {
		return err
	}
	if err = sess.ChannelTyping(string(msg.Portal.ID)); err != nil {
		return fmt.Errorf("failed to send typing notification: %w", err)
	}
	return nil
}
