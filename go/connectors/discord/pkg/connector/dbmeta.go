// Chirper - Discord bridgev2 network connector.
// Copyright (C) 2026 Chirper contributors
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

package connector

import (
	"maunium.net/go/mautrix/bridgev2/database"
)

// UserLoginMetadata is stored per login. The token is a secret and must never be logged.
type UserLoginMetadata struct {
	Token string `json:"token"`
	IsBot bool   `json:"is_bot"`
}

// PortalMetadata describes what kind of Discord channel a portal represents.
type PortalMetadata struct {
	ChannelType int    `json:"channel_type,omitempty"`
	GuildID     string `json:"guild_id,omitempty"`
}

// GhostMetadata holds Discord-specific data about a remote user.
type GhostMetadata struct {
	IsBot bool `json:"is_bot,omitempty"`
}

// MessageMetadata tracks Discord-specific state of a bridged message.
type MessageMetadata struct {
	// LastEditTS is the Discord edit timestamp (unix millis) of the last applied edit.
	LastEditTS int64 `json:"last_edit_ts,omitempty"`
}

func (dc *DiscordConnector) GetDBMetaTypes() database.MetaTypes {
	return database.MetaTypes{
		Portal:    func() any { return &PortalMetadata{} },
		Ghost:     func() any { return &GhostMetadata{} },
		Message:   func() any { return &MessageMetadata{} },
		Reaction:  nil,
		UserLogin: func() any { return &UserLoginMetadata{} },
	}
}
