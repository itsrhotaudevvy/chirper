// SPDX-License-Identifier: AGPL-3.0-or-later

package connector

import (
	"context"
	"errors"
	"fmt"
	"time"

	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/database"
	"maunium.net/go/mautrix/bridgev2/networkid"
	"maunium.net/go/mautrix/event"

	"chirper.dev/connectors/fluxer/pkg/fluxerapi"
)

// maxNonceLength is the longest nonce the create endpoint accepts (docs: http-api/messages.mdx).
const maxNonceLength = 32

var errReactionWithoutKey = errors.New("reaction has no emoji key")

// HandleMatrixMessage sends a Matrix message to Fluxer. Text is sent as Markdown; media is
// downloaded from Matrix and sent as a multipart upload with the caption as content.
func (c *fluxerClient) HandleMatrixMessage(ctx context.Context, msg *bridgev2.MatrixMessage) (*bridgev2.MatrixMessageResponse, error) {
	channelID := string(msg.Portal.ID)
	body := bodyText(msg.Content)
	var sent *fluxerapi.Message
	var err error
	switch {
	case msg.Content.MsgType.IsText():
		sent, err = c.api.CreateMessage(ctx, channelID, fluxerapi.MessageBody{
			Content:          &body,
			Nonce:            nonceFor(msg),
			MessageReference: replyReference(msg),
		})
	case isMediaType(msg.Content.MsgType):
		sent, err = c.sendMedia(ctx, msg, channelID, body)
	default:
		return nil, bridgev2.ErrUnsupportedMessageType
	}
	if err != nil {
		return nil, err
	}
	return &bridgev2.MatrixMessageResponse{
		DB: &database.Message{
			ID:        makeMessageID(sent.ID),
			SenderID:  makeUserID(c.myID()),
			Timestamp: sent.Timestamp,
		},
		StreamOrder: sent.Timestamp.UnixMilli(),
	}, nil
}

func (c *fluxerClient) sendMedia(ctx context.Context, msg *bridgev2.MatrixMessage, channelID, caption string) (*fluxerapi.Message, error) {
	uri := msg.Content.URL
	if msg.Content.File != nil {
		uri = msg.Content.File.URL
	}
	data, err := msg.Portal.Bridge.Bot.DownloadMedia(ctx, uri, msg.Content.File)
	if err != nil {
		return nil, fmt.Errorf("downloading media from Matrix: %w", err)
	}
	name := msg.Content.FileName
	if name == "" {
		name = msg.Content.Body
	}
	mime := ""
	if msg.Content.Info != nil {
		mime = msg.Content.Info.MimeType
	}
	body := fluxerapi.MessageBody{MessageReference: replyReference(msg), Nonce: nonceFor(msg)}
	if caption != "" {
		body.Content = &caption
	}
	upload := fluxerapi.Upload{
		Meta: fluxerapi.AttachmentMeta{ID: 0, Filename: name, ContentType: mime},
		Data: data,
	}
	return c.api.CreateMessage(ctx, channelID, body, upload)
}

func isMediaType(t event.MessageType) bool {
	return t == event.MsgImage || t == event.MsgVideo || t == event.MsgAudio || t == event.MsgFile
}

// nonceFor passes the transaction ID as the message nonce, so that retries do not duplicate.
func nonceFor(msg *bridgev2.MatrixMessage) string {
	n := string(msg.InputTransactionID)
	if len(n) > maxNonceLength {
		return ""
	}
	return n
}

func replyReference(msg *bridgev2.MatrixMessage) *fluxerapi.MessageReference {
	if msg.ReplyTo == nil {
		return nil
	}
	return &fluxerapi.MessageReference{MessageID: string(msg.ReplyTo.ID), ChannelID: string(msg.Portal.ID), Type: 0}
}

// HandleMatrixEdit edits the text of a message the account sent.
func (c *fluxerClient) HandleMatrixEdit(ctx context.Context, msg *bridgev2.MatrixEdit) error {
	_, err := c.api.EditMessage(ctx, string(msg.Portal.ID), string(msg.EditTarget.ID), bodyText(msg.Content))
	return err
}

// HandleMatrixMessageRemove deletes a message from Fluxer.
func (c *fluxerClient) HandleMatrixMessageRemove(ctx context.Context, msg *bridgev2.MatrixMessageRemove) error {
	return c.api.DeleteMessage(ctx, string(msg.Portal.ID), string(msg.TargetMessage.ID))
}

// PreHandleMatrixReaction fills in the sender and emoji that the reaction is stored under.
func (c *fluxerClient) PreHandleMatrixReaction(_ context.Context, msg *bridgev2.MatrixReaction) (bridgev2.MatrixReactionPreResponse, error) {
	key := reactionKey(msg)
	if key == "" {
		return bridgev2.MatrixReactionPreResponse{}, errReactionWithoutKey
	}
	return bridgev2.MatrixReactionPreResponse{
		SenderID: makeUserID(c.myID()),
		EmojiID:  networkid.EmojiID(key),
		Emoji:    key,
	}, nil
}

// HandleMatrixReaction adds the account's reaction on Fluxer.
func (c *fluxerClient) HandleMatrixReaction(ctx context.Context, msg *bridgev2.MatrixReaction) (*database.Reaction, error) {
	key := reactionKey(msg)
	if key == "" {
		return nil, errReactionWithoutKey
	}
	if err := c.api.AddReaction(ctx, string(msg.Portal.ID), string(msg.TargetMessage.ID), key); err != nil {
		return nil, err
	}
	return &database.Reaction{
		MessageID:     msg.TargetMessage.ID,
		MessagePartID: msg.TargetMessage.PartID,
		SenderID:      makeUserID(c.myID()),
		EmojiID:       networkid.EmojiID(key),
		Emoji:         key,
		Timestamp:     time.Now(),
	}, nil
}

// HandleMatrixReactionRemove removes the account's reaction on Fluxer.
func (c *fluxerClient) HandleMatrixReactionRemove(ctx context.Context, msg *bridgev2.MatrixReactionRemove) error {
	return c.api.RemoveOwnReaction(ctx, string(msg.Portal.ID), string(msg.TargetReaction.MessageID), msg.TargetReaction.Emoji)
}

func reactionKey(msg *bridgev2.MatrixReaction) string {
	return msg.Content.RelatesTo.Key
}

// HandleMatrixTyping shows the typing indicator. Only text typing is supported by Fluxer.
func (c *fluxerClient) HandleMatrixTyping(ctx context.Context, msg *bridgev2.MatrixTyping) error {
	if !msg.IsTyping || msg.Type != bridgev2.TypingTypeText {
		return nil
	}
	return c.api.Typing(ctx, string(msg.Portal.ID))
}

// HandleMatrixReadReceipt marks a channel as read on Fluxer. Bots cannot acknowledge messages
// (docs: http-api/messages.mdx "Acknowledge message"), so their receipts are dropped.
func (c *fluxerClient) HandleMatrixReadReceipt(ctx context.Context, msg *bridgev2.MatrixReadReceipt) error {
	if c.cred.Bot || msg.ExactMessage == nil {
		return nil
	}
	return c.api.AckMessage(ctx, string(msg.Portal.ID), string(msg.ExactMessage.ID))
}
