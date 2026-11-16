// SPDX-License-Identifier: AGPL-3.0-or-later

package connector

import (
	"context"
	"slices"

	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/networkid"

	"chirper.dev/connectors/fluxer/pkg/fluxerapi"
)

// maxBackfillPage is the largest page the list messages endpoint returns.
const maxBackfillPage = 100

// FetchMessages pages through channel history. Backward pages use "before" the cursor (or the
// anchor message); forward pages use "after" the anchor. The API lists newest first, so pages are
// reversed into chronological order.
func (c *fluxerClient) FetchMessages(ctx context.Context, params bridgev2.FetchMessagesParams) (*bridgev2.FetchMessagesResponse, error) {
	count := min(max(params.Count, 1), maxBackfillPage)
	q := fluxerapi.MessageQuery{Limit: count}
	if params.Forward {
		if params.AnchorMessage != nil {
			q.After = string(params.AnchorMessage.ID)
		}
	} else {
		switch {
		case params.Cursor != "":
			q.Before = string(params.Cursor)
		case params.AnchorMessage != nil:
			q.Before = string(params.AnchorMessage.ID)
		}
	}
	msgs, err := c.api.Messages(ctx, string(params.Portal.ID), q)
	if err != nil {
		return nil, err
	}
	slices.Reverse(msgs)

	backfill := make([]*bridgev2.BackfillMessage, 0, len(msgs))
	for i := range msgs {
		m := &msgs[i]
		converted, err := c.convertMessage(ctx, params.Portal, params.Portal.Bridge.Bot, m)
		if err != nil {
			continue
		}
		backfill = append(backfill, &bridgev2.BackfillMessage{
			ConvertedMessage: converted,
			Sender:           c.senderOf(m.Author.ID),
			ID:               makeMessageID(m.ID),
			Timestamp:        m.Timestamp,
			StreamOrder:      m.Timestamp.UnixMilli(),
		})
	}
	resp := &bridgev2.FetchMessagesResponse{
		Messages: backfill,
		Forward:  params.Forward,
		HasMore:  len(msgs) == count,
	}
	if len(msgs) > 0 && !params.Forward {
		resp.Cursor = networkid.PaginationCursor(msgs[0].ID)
	}
	return resp, nil
}
