// SPDX-License-Identifier: AGPL-3.0-or-later

package connector

import (
	"context"
	"time"

	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/event"
)

var generalCaps = &bridgev2.NetworkGeneralCapabilities{
	DisappearingMessages: false,
	AggressiveUpdateInfo: false,
	// Fluxer typing indicators expire after about 10 seconds, so they are repeated.
	TypingRepeatInterval: 8 * time.Second,
}

// maxUploadBytes is a conservative per-file cap for outgoing media. Instances publish their real
// limit in discovery (max_attachment_file_size); 25 MiB is the non-premium default on instances with premium tiers.
const maxUploadBytes = 25 << 20

// roomCaps describes what a Fluxer chat supports. Text is limited server-side by max_message_length,
// which is 4000 characters on the default instance.
var roomCaps = &event.RoomFeatures{
	ID:                  "org.chirper.fluxer.capabilities.2026_11_16",
	MaxTextLength:       4000,
	Reply:               event.CapLevelFullySupported,
	Edit:                event.CapLevelFullySupported,
	Delete:              event.CapLevelFullySupported,
	Reaction:            event.CapLevelFullySupported,
	ReadReceipts:        true,
	TypingNotifications: true,
	File: event.FileFeatureMap{
		event.MsgImage: fileFeatures(),
		event.MsgVideo: fileFeatures(),
		event.MsgAudio: fileFeatures(),
		event.MsgFile:  fileFeatures(),
	},
}

func fileFeatures() *event.FileFeatures {
	return &event.FileFeatures{
		MimeTypes: map[string]event.CapabilitySupportLevel{"*/*": event.CapLevelFullySupported},
		Caption:   event.CapLevelFullySupported,
		MaxSize:   maxUploadBytes,
	}
}

// GetCapabilities returns the room features; they do not vary per chat.
func (c *fluxerClient) GetCapabilities(ctx context.Context, portal *bridgev2.Portal) *event.RoomFeatures {
	return roomCaps
}
