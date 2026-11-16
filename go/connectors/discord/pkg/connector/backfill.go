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
	"fmt"

	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/database"
	"maunium.net/go/mautrix/bridgev2/networkid"
)

// GetBackfillMaxBatchCount returns the page size configured for history fetches.
func (c *DiscordClient) GetBackfillMaxBatchCount(ctx context.Context, portal *bridgev2.Portal, task *database.BackfillTask) int {
	return c.Main.Config.Backfill.MaxBatchCount
}

// FetchMessages pages through channel history. Discord returns newest first; the result is
// chronological and the cursor is the message ID to continue from.
func (c *DiscordClient) FetchMessages(ctx context.Context, params bridgev2.FetchMessagesParams) (*bridgev2.FetchMessagesResponse, error) {
	sess, err := c.currentSession()
	if err != nil {
		return nil, err
	}
	count := c.Main.Config.Backfill.MaxBatchCount
	if params.Count > 0 && params.Count < count {
		count = params.Count
	}
	before, after := paginationAnchors(params)
	msgs, err := sess.ChannelMessages(string(params.Portal.ID), count, before, after, "")
	if err != nil {
		return nil, fmt.Errorf("failed to fetch messages from Discord: %w", err)
	}
	resp := &bridgev2.FetchMessagesResponse{
		Forward: params.Forward,
		HasMore: len(msgs) >= count,
	}
	intent := params.Portal.Bridge.Bot
	for i := len(msgs) - 1; i >= 0; i-- {
		msg := msgs[i]
		if msg.Author == nil {
			continue
		}
		converted, err := c.convertMessage(ctx, params.Portal, intent, msg)
		if err != nil {
			return nil, fmt.Errorf("failed to convert message %s: %w", msg.ID, err)
		}
		resp.Messages = append(resp.Messages, &bridgev2.BackfillMessage{
			ConvertedMessage: converted,
			Sender:           c.makeSender(msg.Author.ID),
			ID:               makeMessageID(msg.ID),
			Timestamp:        msg.Timestamp,
			StreamOrder:      msg.Timestamp.UnixMicro(),
		})
	}
	if len(msgs) > 0 {
		if params.Forward {
			resp.Cursor = networkid.PaginationCursor(msgs[0].ID)
		} else {
			resp.Cursor = networkid.PaginationCursor(msgs[len(msgs)-1].ID)
		}
	}
	return resp, nil
}

// paginationAnchors picks the Discord before/after parameters from the cursor or anchor message.
func paginationAnchors(params bridgev2.FetchMessagesParams) (before, after string) {
	cursor := string(params.Cursor)
	anchor := ""
	if params.AnchorMessage != nil {
		anchor = string(params.AnchorMessage.ID)
	}
	switch {
	case params.Forward && cursor != "":
		return "", cursor
	case params.Forward:
		return "", anchor
	case cursor != "":
		return cursor, ""
	default:
		return anchor, ""
	}
}
