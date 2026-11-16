// Chirper - Discord bridgev2 network connector.
// Copyright (C) 2026 Chirper contributors
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

package connector

import (
	"testing"

	"github.com/bwmarrin/discordgo"
	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/database"
	"maunium.net/go/mautrix/bridgev2/networkid"
)

func emojiForTest(name, emojiID string) discordgo.Emoji {
	return discordgo.Emoji{Name: name, ID: emojiID}
}

func TestReactionEmojiRoundTrip(t *testing.T) {
	tests := []struct {
		name     string
		emojiID  networkid.EmojiID
		emoji    string
		wantSend string
	}{
		{name: "unicode", emojiID: "👍", emoji: "👍", wantSend: "👍"},
		{name: "custom", emojiID: "custom:999", emoji: ":wave:", wantSend: "wave:999"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := discordReactionKey(tt.emojiID, tt.emoji); got != tt.wantSend {
				t.Errorf("discordReactionKey(%q, %q) = %q, want %q", tt.emojiID, tt.emoji, got, tt.wantSend)
			}
		})
	}
}

func TestReactionEmojiFromDiscord(t *testing.T) {
	key, emojiID := reactionEmoji(emojiForTest("wave", "999"))
	if key != ":wave:" || emojiID != "custom:999" {
		t.Errorf("custom emoji = %q, %q", key, emojiID)
	}
	key, emojiID = reactionEmoji(emojiForTest("🎉", ""))
	if key != "🎉" || emojiID != "🎉" {
		t.Errorf("unicode emoji = %q, %q", key, emojiID)
	}
}

func TestPartIDs(t *testing.T) {
	if makeTextPartID() != "" {
		t.Error("text part must use the empty part ID")
	}
	if got := makeAttachmentPartID("55"); got != "att-55" {
		t.Errorf("attachment part ID = %q", got)
	}
	if got := makeEmbedPartID(2); got != "embed-2" {
		t.Errorf("embed part ID = %q", got)
	}
}

func TestPortalKeysHaveNoReceiver(t *testing.T) {
	if key := makePortalKey("123"); key.ID != "123" || key.Receiver != "" {
		t.Errorf("channel portal key = %+v", key)
	}
	if key := makeGuildPortalKey("456"); key.ID != "456" || key.Receiver != "" {
		t.Errorf("guild portal key = %+v", key)
	}
}

func TestPaginationAnchors(t *testing.T) {
	anchor := &database.Message{ID: "7"}
	tests := []struct {
		name       string
		params     bridgev2.FetchMessagesParams
		wantBefore string
		wantAfter  string
	}{
		{name: "backward from cursor", params: bridgev2.FetchMessagesParams{Cursor: "10"}, wantBefore: "10"},
		{name: "forward from cursor", params: bridgev2.FetchMessagesParams{Forward: true, Cursor: "10"}, wantAfter: "10"},
		{name: "backward from anchor", params: bridgev2.FetchMessagesParams{AnchorMessage: anchor}, wantBefore: "7"},
		{name: "forward from anchor", params: bridgev2.FetchMessagesParams{Forward: true, AnchorMessage: anchor}, wantAfter: "7"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			before, after := paginationAnchors(tt.params)
			if before != tt.wantBefore || after != tt.wantAfter {
				t.Errorf("anchors = (%q, %q), want (%q, %q)", before, after, tt.wantBefore, tt.wantAfter)
			}
		})
	}
}
