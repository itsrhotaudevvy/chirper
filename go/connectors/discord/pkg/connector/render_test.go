// Chirper - Discord bridgev2 network connector.
// Copyright (C) 2026 Chirper contributors
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

package connector

import (
	"encoding/json"
	"strings"
	"testing"
	"time"

	"github.com/bwmarrin/discordgo"
	"maunium.net/go/mautrix/bridgev2/database"
)

const componentsJSON = `{
  "id": "1",
  "channel_id": "2",
  "content": "pick one",
  "author": {"id": "3", "username": "bot"},
  "components": [
    {"type": 1, "components": [
      {"type": 2, "style": 1, "label": "Approve", "custom_id": "approve:42"},
      {"type": 2, "style": 5, "label": "Docs", "url": "https://example.com/docs"}
    ]},
    {"type": 1, "components": [
      {"type": 3, "custom_id": "pick", "placeholder": "Choose", "options": [
        {"label": "Red", "value": "red"}, {"label": "Blue", "value": "blue"}
      ]}
    ]}
  ]
}`

// TestComponentsFromGateway checks that components decoded from a real Discord payload render,
// since the renderer type-switches on the concrete discordgo component types.
func TestComponentsFromGateway(t *testing.T) {
	var msg discordgo.Message
	if err := json.Unmarshal([]byte(componentsJSON), &msg); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	htmlList, plain, raw, err := renderComponents(msg.Components)
	if err != nil {
		t.Fatalf("renderComponents: %v", err)
	}
	for _, want := range []string{
		"<strong>[Approve]</strong>",
		`<a href="https://example.com/docs">Docs</a>`,
		"<strong>[Choose]</strong> Red, Blue",
	} {
		if !strings.Contains(htmlList, want) {
			t.Errorf("html list %q missing %q", htmlList, want)
		}
	}
	if !strings.Contains(plain, "- [Approve]") {
		t.Errorf("plain text %q missing button", plain)
	}
	if !strings.Contains(string(raw), `"custom_id":"approve:42"`) {
		t.Errorf("raw components lost the custom ID: %s", raw)
	}
}

func TestRenderComponentsEmpty(t *testing.T) {
	htmlList, plain, raw, err := renderComponents(nil)
	if err != nil || htmlList != "" || plain != "" || raw != nil {
		t.Errorf("empty components = (%q, %q, %s, %v)", htmlList, plain, raw, err)
	}
}

func TestRenderEmbed(t *testing.T) {
	embed := &discordgo.MessageEmbed{
		Title:       "Release",
		URL:         "https://example.com/r",
		Description: "**new** build",
		Fields:      []*discordgo.MessageEmbedField{{Name: "Version", Value: "1.2"}},
		Footer:      &discordgo.MessageEmbedFooter{Text: "footer"},
	}
	content := renderEmbed(embed, nil, testGhosts)
	if content == nil {
		t.Fatal("expected notice content")
	}
	if !strings.Contains(content.FormattedBody, `<a href="https://example.com/r">Release</a>`) {
		t.Errorf("title link missing: %s", content.FormattedBody)
	}
	if !strings.Contains(content.FormattedBody, "<strong>new</strong>") {
		t.Errorf("description not rendered as markdown: %s", content.FormattedBody)
	}
	if !strings.Contains(content.Body, "Version: 1.2") {
		t.Errorf("plain body missing field: %q", content.Body)
	}
}

func TestRenderEmbedEmpty(t *testing.T) {
	if content := renderEmbed(&discordgo.MessageEmbed{}, nil, testGhosts); content != nil {
		t.Errorf("empty embed rendered as %v", content)
	}
}

func TestCheckEditIsNew(t *testing.T) {
	edited := int64(2000)
	msg := &discordgo.Message{}
	msg.EditedTimestamp = timePtr(edited)
	existing := []*database.Message{{PartID: makeTextPartID(), Metadata: &MessageMetadata{LastEditTS: 2000}}}
	if err := checkEditIsNew(existing, msg); err == nil {
		t.Error("an edit equal to the last applied edit must be ignored")
	}
	newer := &discordgo.Message{}
	newer.EditedTimestamp = timePtr(3000)
	if err := checkEditIsNew(existing, newer); err != nil {
		t.Errorf("newer edit rejected: %v", err)
	}
}

func timePtr(unixMillis int64) *time.Time {
	t := time.UnixMilli(unixMillis)
	return &t
}
