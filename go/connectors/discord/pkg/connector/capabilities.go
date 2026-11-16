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

	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/event"
)

// discordMaxMessageLength is the content limit for accounts without Discord Nitro.
const discordMaxMessageLength = 2000

// GetCapabilities describes what a room supports. Read receipts are only available to user accounts,
// because bots have no read state on Discord.
func (c *DiscordClient) GetCapabilities(ctx context.Context, portal *bridgev2.Portal) *event.RoomFeatures {
	return &event.RoomFeatures{
		MaxTextLength:        discordMaxMessageLength,
		Edit:                 event.CapLevelFullySupported,
		Delete:               event.CapLevelFullySupported,
		Reply:                event.CapLevelFullySupported,
		Reaction:             event.CapLevelFullySupported,
		CustomEmojiReactions: c.Main.Config.CustomEmojiReactions,
		TypingNotifications:  true,
		ReadReceipts:         !c.isBot(),
	}
}
