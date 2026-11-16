// SPDX-License-Identifier: AGPL-3.0-or-later

package connector

import (
	"context"
	"errors"
	"strings"

	"github.com/rs/zerolog"
	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/database"
	"maunium.net/go/mautrix/bridgev2/networkid"
	"maunium.net/go/mautrix/event"

	"chirper.dev/connectors/fluxer/pkg/fluxerapi"
)

// Message types that carry a user's text (docs: http-api/messages.mdx "Message types").
const (
	messageTypeDefault = 0
	messageTypeReply   = 19
)

const unsupportedNotice = "[unsupported message]"

// convertMessage turns a Fluxer message into bridgev2 parts: one text part (PartID "") when there
// is text, and one part per attachment. It is used for live messages and for backfill.
func (c *fluxerClient) convertMessage(ctx context.Context, portal *bridgev2.Portal, intent bridgev2.MatrixAPI, msg *fluxerapi.Message) (*bridgev2.ConvertedMessage, error) {
	var parts []*bridgev2.ConvertedMessagePart
	if text := textPart(msg); text != nil {
		parts = append(parts, text)
	}
	for _, att := range msg.Attachments {
		part, err := c.attachmentPart(ctx, portal, intent, att)
		if err != nil {
			zerolog.Ctx(ctx).Warn().Err(err).Str("attachment_id", att.ID).Msg("Attachment could not be bridged")
			part = noticePart(makePartID(att.ID), "[attachment unavailable]")
		}
		parts = append(parts, part)
	}
	if len(parts) == 0 {
		parts = append(parts, noticePart("", unsupportedNotice))
	}
	converted := &bridgev2.ConvertedMessage{Parts: parts}
	if ref := msg.MessageReference; ref != nil && ref.MessageID != "" && ref.Type != 1 {
		converted.ReplyTo = &networkid.MessageOptionalPartID{MessageID: makeMessageID(ref.MessageID)}
	}
	return converted, nil
}

// textPart is the message's text as m.text, or a notice for system messages. It returns nil
// when the message has no text.
func textPart(msg *fluxerapi.Message) *bridgev2.ConvertedMessagePart {
	switch msg.Type {
	case messageTypeDefault, messageTypeReply:
	default:
		return noticePart("", "[system message]")
	}
	if msg.Content == "" {
		return nil
	}
	return &bridgev2.ConvertedMessagePart{
		Type:    event.EventMessage,
		Content: &event.MessageEventContent{MsgType: event.MsgText, Body: msg.Content},
	}
}

func noticePart(id networkid.PartID, body string) *bridgev2.ConvertedMessagePart {
	return &bridgev2.ConvertedMessagePart{
		ID:      id,
		Type:    event.EventMessage,
		Content: &event.MessageEventContent{MsgType: event.MsgNotice, Body: body},
	}
}

// attachmentPart downloads an attachment and uploads it to Matrix through the sender's intent.
func (c *fluxerClient) attachmentPart(ctx context.Context, portal *bridgev2.Portal, intent bridgev2.MatrixAPI, att fluxerapi.Attachment) (*bridgev2.ConvertedMessagePart, error) {
	if att.URL == nil || att.Expired {
		return nil, errAttachmentExpired
	}
	data, err := downloadBytes(ctx, c.main.HTTP, *att.URL, maxMediaBytes)
	if err != nil {
		return nil, err
	}
	mxc, file, err := intent.UploadMedia(ctx, portal.MXID, data, att.Filename, att.ContentType)
	if err != nil {
		return nil, err
	}
	info := &event.FileInfo{MimeType: att.ContentType, Size: int(att.Size)}
	if att.Width != nil {
		info.Width = *att.Width
	}
	if att.Height != nil {
		info.Height = *att.Height
	}
	if att.Duration != nil {
		info.Duration = *att.Duration * 1000
	}
	content := &event.MessageEventContent{
		MsgType:  msgTypeFor(att.ContentType),
		Body:     att.Filename,
		FileName: att.Filename,
		URL:      mxc,
		File:     file,
		Info:     info,
	}
	return &bridgev2.ConvertedMessagePart{
		ID:      makePartID(att.ID),
		Type:    event.EventMessage,
		Content: content,
	}, nil
}

// convertEdit updates the text part of a stored message after an edit. Attachments are not edited.
func (c *fluxerClient) convertEdit(ctx context.Context, portal *bridgev2.Portal, intent bridgev2.MatrixAPI, existing []*database.Message, msg *fluxerapi.Message) (*bridgev2.ConvertedEdit, error) {
	edit := &bridgev2.ConvertedEdit{}
	for _, part := range existing {
		if part.PartID != "" {
			continue
		}
		newPart := textPart(msg)
		if newPart == nil {
			continue
		}
		edit.ModifiedParts = append(edit.ModifiedParts, &bridgev2.ConvertedEditPart{
			Part:    part,
			Type:    newPart.Type,
			Content: newPart.Content,
		})
	}
	return edit, nil
}

// msgTypeFor maps a MIME type to the Matrix message type.
func msgTypeFor(mimeType string) event.MessageType {
	switch {
	case strings.HasPrefix(mimeType, "image/"):
		return event.MsgImage
	case strings.HasPrefix(mimeType, "video/"):
		return event.MsgVideo
	case strings.HasPrefix(mimeType, "audio/"):
		return event.MsgAudio
	default:
		return event.MsgFile
	}
}

var errAttachmentExpired = errors.New("attachment URL has expired")
