// Chirper - Discord bridgev2 network connector.
// Copyright (C) 2026 Chirper contributors
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

package connector

import (
	"context"
	"encoding/json"
	"fmt"
	"html"
	"strings"

	"github.com/bwmarrin/discordgo"
	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/database"
	"maunium.net/go/mautrix/bridgev2/networkid"
	"maunium.net/go/mautrix/event"
	"maunium.net/go/mautrix/id"
)

const (
	discordStickerFormatPNG  = 1
	discordStickerFormatAPNG = 2
	discordStickerFormatGIF  = 4
	discordStickerCDN        = "https://media.discordapp.net/stickers"
)

// convertMessage turns a Discord message into Matrix parts: text (with components), attachments,
// embeds and stickers. Every part has a stable part ID so edits can be applied part by part.
func (c *DiscordClient) convertMessage(ctx context.Context, portal *bridgev2.Portal, intent bridgev2.MatrixAPI, msg *discordgo.Message) (*bridgev2.ConvertedMessage, error) {
	ghosts := c.Main.br.Matrix
	names := mentionNamesFor(msg)
	var parts []*bridgev2.ConvertedMessagePart

	text, err := c.textPart(msg, names, ghosts)
	if err != nil {
		return nil, err
	}
	if text != nil {
		parts = append(parts, text)
	}
	for _, att := range msg.Attachments {
		parts = append(parts, c.attachmentPart(ctx, portal, intent, att))
	}
	for i, embed := range msg.Embeds {
		if content := renderEmbed(embed, names, ghosts); content != nil {
			parts = append(parts, &bridgev2.ConvertedMessagePart{
				ID:      makeEmbedPartID(i),
				Type:    event.EventMessage,
				Content: content,
			})
		}
	}
	for _, sticker := range msg.StickerItems {
		parts = append(parts, c.stickerPart(ctx, portal, intent, sticker))
	}
	if len(parts) == 0 {
		parts = append(parts, noticePart(makeTextPartID(), "Unsupported Discord message"))
	}
	return &bridgev2.ConvertedMessage{
		ReplyTo: replyTarget(msg),
		Parts:   parts,
	}, nil
}

func mentionNamesFor(msg *discordgo.Message) mentionNames {
	names := make(mentionNames, len(msg.Mentions))
	for _, user := range msg.Mentions {
		names[user.ID] = displayName(user)
	}
	return names
}

func replyTarget(msg *discordgo.Message) *networkid.MessageOptionalPartID {
	if msg.MessageReference == nil || msg.MessageReference.MessageID == "" {
		return nil
	}
	return &networkid.MessageOptionalPartID{MessageID: makeMessageID(msg.MessageReference.MessageID)}
}

// editTimestampMillis returns the message's last edit time in unix millis, or 0 if never edited.
func editTimestampMillis(msg *discordgo.Message) int64 {
	if msg.EditedTimestamp == nil {
		return 0
	}
	return msg.EditedTimestamp.UnixMilli()
}

func noticePart(partID networkid.PartID, body string) *bridgev2.ConvertedMessagePart {
	return &bridgev2.ConvertedMessagePart{
		ID:      partID,
		Type:    event.EventMessage,
		Content: &event.MessageEventContent{MsgType: event.MsgNotice, Body: body},
	}
}

// textPart builds the text body, including a list of message components when present.
// The text part carries the message's edit timestamp so later edits can be ordered.
func (c *DiscordClient) textPart(msg *discordgo.Message, names mentionNames, ghosts ghostMapper) (*bridgev2.ConvertedMessagePart, error) {
	componentsHTML, componentsPlain, raw, err := renderComponents(msg.Components)
	if err != nil {
		return nil, err
	}
	if msg.Content == "" && componentsHTML == "" {
		return nil, nil
	}
	body := msg.Content
	var formatted string
	var mentioned []string
	if msg.Content != "" {
		formatted, mentioned = discordToHTML(msg.Content, names, ghosts)
	}
	if componentsHTML != "" {
		if body != "" {
			body += "\n"
			formatted += "<br>"
		}
		body += componentsPlain
		formatted += componentsHTML
	}
	mentionIDs := make([]id.UserID, 0, len(mentioned))
	for _, userID := range mentioned {
		mentionIDs = append(mentionIDs, ghosts.FormatGhostMXID(networkid.UserID(userID)))
	}
	part := &bridgev2.ConvertedMessagePart{
		ID:   makeTextPartID(),
		Type: event.EventMessage,
		Content: &event.MessageEventContent{
			MsgType:       event.MsgText,
			Body:          body,
			Format:        event.FormatHTML,
			FormattedBody: formatted,
			Mentions:      &event.Mentions{UserIDs: mentionIDs},
		},
		DBMetadata: &MessageMetadata{LastEditTS: editTimestampMillis(msg)},
	}
	if raw != nil {
		part.Extra = map[string]any{componentsExtraKey: json.RawMessage(raw)}
	}
	return part, nil
}

// attachmentPart downloads a Discord attachment and uploads it to Matrix. If that fails, the part
// becomes a notice that links to the Discord CDN so the content is still reachable.
func (c *DiscordClient) attachmentPart(ctx context.Context, portal *bridgev2.Portal, intent bridgev2.MatrixAPI, att *discordgo.MessageAttachment) *bridgev2.ConvertedMessagePart {
	partID := makeAttachmentPartID(att.ID)
	data, downloadType, err := c.Main.download(ctx, att.URL)
	if err != nil {
		return attachmentFallbackPart(partID, att, err)
	}
	mimeType := att.ContentType
	if mimeType == "" {
		mimeType = downloadType
	}
	if mimeType == "" {
		mimeType = "application/octet-stream"
	}
	mxc, file, err := intent.UploadMedia(ctx, portal.MXID, data, att.Filename, mimeType)
	if err != nil {
		return attachmentFallbackPart(partID, att, err)
	}
	return &bridgev2.ConvertedMessagePart{
		ID:   partID,
		Type: event.EventMessage,
		Content: &event.MessageEventContent{
			MsgType: msgTypeForMIME(mimeType),
			Body:    att.Filename,
			URL:     mxc,
			File:    file,
			Info: &event.FileInfo{
				MimeType: mimeType,
				Size:     len(data),
				Width:    att.Width,
				Height:   att.Height,
			},
		},
	}
}

func attachmentFallbackPart(partID networkid.PartID, att *discordgo.MessageAttachment, err error) *bridgev2.ConvertedMessagePart {
	name := att.Filename
	if name == "" {
		name = "attachment"
	}
	return &bridgev2.ConvertedMessagePart{
		ID:   partID,
		Type: event.EventMessage,
		Content: &event.MessageEventContent{
			MsgType:       event.MsgNotice,
			Body:          fmt.Sprintf("%s (could not be bridged: %v): %s", name, err, att.URL),
			Format:        event.FormatHTML,
			FormattedBody: fmt.Sprintf(`%s (could not be bridged: %s): <a href="%s">%s</a>`, html.EscapeString(name), html.EscapeString(err.Error()), html.EscapeString(att.URL), html.EscapeString(att.URL)),
		},
	}
}

func msgTypeForMIME(mimeType string) event.MessageType {
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

// stickerPart sends PNG, APNG and GIF stickers as Matrix stickers. Lottie stickers are rendered
// as a text placeholder because they need a vector renderer.
func (c *DiscordClient) stickerPart(ctx context.Context, portal *bridgev2.Portal, intent bridgev2.MatrixAPI, sticker *discordgo.StickerItem) *bridgev2.ConvertedMessagePart {
	partID := makeStickerPartID(sticker.ID)
	ext, mimeType := "png", "image/png"
	switch int(sticker.FormatType) {
	case discordStickerFormatPNG, discordStickerFormatAPNG:
	case discordStickerFormatGIF:
		ext, mimeType = "gif", "image/gif"
	default:
		return noticePart(partID, fmt.Sprintf("[sticker: %s]", sticker.Name))
	}
	url := fmt.Sprintf("%s/%s.%s?size=320", discordStickerCDN, sticker.ID, ext)
	data, _, err := c.Main.download(ctx, url)
	if err != nil {
		return noticePart(partID, fmt.Sprintf("[sticker: %s] (could not be bridged: %v)", sticker.Name, err))
	}
	mxc, _, err := intent.UploadMedia(ctx, portal.MXID, data, sticker.Name+"."+ext, mimeType)
	if err != nil {
		return noticePart(partID, fmt.Sprintf("[sticker: %s] (could not be bridged: %v)", sticker.Name, err))
	}
	return &bridgev2.ConvertedMessagePart{
		ID:   partID,
		Type: event.EventSticker,
		Content: &event.MessageEventContent{
			Body: sticker.Name,
			URL:  mxc,
			Info: &event.FileInfo{MimeType: mimeType, Size: len(data)},
		},
	}
}

// convertEdit applies a Discord message edit part by part. Edits older than the last applied edit
// are ignored, because Discord can deliver updates out of order.
func (c *DiscordClient) convertEdit(ctx context.Context, portal *bridgev2.Portal, intent bridgev2.MatrixAPI, existing []*database.Message, msg *discordgo.Message) (*bridgev2.ConvertedEdit, error) {
	if err := checkEditIsNew(existing, msg); err != nil {
		return nil, err
	}
	converted, err := c.convertMessage(ctx, portal, intent, msg)
	if err != nil {
		return nil, err
	}
	byPart := make(map[networkid.PartID]*database.Message, len(existing))
	for _, part := range existing {
		byPart[part.PartID] = part
	}
	edit := &bridgev2.ConvertedEdit{}
	kept := make(map[networkid.PartID]bool, len(converted.Parts))
	var added []*bridgev2.ConvertedMessagePart
	for _, part := range converted.Parts {
		old, found := byPart[part.ID]
		if !found {
			added = append(added, part)
			continue
		}
		kept[part.ID] = true
		edit.ModifiedParts = append(edit.ModifiedParts, part.ToEditPart(old))
	}
	for _, old := range existing {
		if !kept[old.PartID] {
			edit.DeletedParts = append(edit.DeletedParts, old)
		}
	}
	if len(added) > 0 {
		edit.AddedParts = &bridgev2.ConvertedMessage{Parts: added}
	}
	return edit, nil
}

func checkEditIsNew(existing []*database.Message, msg *discordgo.Message) error {
	editTS := editTimestampMillis(msg)
	for _, part := range existing {
		meta, ok := part.Metadata.(*MessageMetadata)
		if ok && part.PartID == makeTextPartID() && meta.LastEditTS >= editTS {
			return fmt.Errorf("%w: last applied edit is not older than this one", bridgev2.ErrIgnoringRemoteEvent)
		}
	}
	return nil
}
