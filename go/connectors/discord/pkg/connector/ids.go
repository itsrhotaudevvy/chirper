// Chirper - Discord bridgev2 network connector.
// Copyright (C) 2026 Chirper contributors
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

package connector

import (
	"fmt"

	"maunium.net/go/mautrix/bridgev2/networkid"
)

// Discord snowflakes are globally unique across users, guilds, channels and messages,
// so the raw snowflake is used directly as the portal, ghost and message ID.
// Portal keys have an empty receiver: a channel is the same room for every login that can see it.

func makeUserID(userID string) networkid.UserID {
	return networkid.UserID(userID)
}

func makeUserLoginID(userID string) networkid.UserLoginID {
	return networkid.UserLoginID(userID)
}

func makePortalKey(channelID string) networkid.PortalKey {
	return networkid.PortalKey{ID: networkid.PortalID(channelID)}
}

func makeGuildPortalKey(guildID string) networkid.PortalKey {
	return networkid.PortalKey{ID: networkid.PortalID(guildID)}
}

func makeMessageID(messageID string) networkid.MessageID {
	return networkid.MessageID(messageID)
}

// makePartID returns the part ID for the text body of a message.
func makeTextPartID() networkid.PartID {
	return ""
}

func makeAttachmentPartID(attachmentID string) networkid.PartID {
	return networkid.PartID("att-" + attachmentID)
}

func makeEmbedPartID(index int) networkid.PartID {
	return networkid.PartID(fmt.Sprintf("embed-%d", index))
}

func makeStickerPartID(stickerID string) networkid.PartID {
	return networkid.PartID("sticker-" + stickerID)
}
