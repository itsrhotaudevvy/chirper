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

	"github.com/bwmarrin/discordgo"
)

func TestFindComponentType(t *testing.T) {
	var msg discordgo.Message
	if err := json.Unmarshal([]byte(componentsJSON), &msg); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	tests := []struct {
		customID string
		want     discordgo.ComponentType
		wantErr  bool
	}{
		{customID: "approve:42", want: discordgo.ButtonComponent},
		{customID: "pick", want: discordgo.SelectMenuComponent},
		{customID: "missing", wantErr: true},
	}
	for _, tt := range tests {
		got, err := findComponentType(msg.Components, tt.customID)
		if (err != nil) != tt.wantErr {
			t.Errorf("findComponentType(%q) error = %v, wantErr %v", tt.customID, err, tt.wantErr)
		}
		if got != tt.want {
			t.Errorf("findComponentType(%q) = %v, want %v", tt.customID, got, tt.want)
		}
	}
}

func TestBuildCommandOptions(t *testing.T) {
	declared := []*discordgo.ApplicationCommandOption{
		{Name: "query", Type: discordgo.ApplicationCommandOptionString, Required: true},
		{Name: "limit", Type: discordgo.ApplicationCommandOptionInteger},
	}
	inputs, err := buildCommandOptions(declared, map[string]any{"query": "cats", "limit": 3})
	if err != nil {
		t.Fatalf("valid options rejected: %v", err)
	}
	if len(inputs) != 2 {
		t.Fatalf("got %d inputs, want 2", len(inputs))
	}
	if _, err = buildCommandOptions(declared, map[string]any{"limit": 3}); err == nil {
		t.Error("missing required option accepted")
	}
	if _, err = buildCommandOptions(declared, map[string]any{"query": "x", "typo": 1}); err == nil {
		t.Error("unknown option accepted")
	}
}

func TestInteractionPayloadShape(t *testing.T) {
	raw, err := json.Marshal(interactionPayload{
		Type:          interactionTypeComponent,
		Nonce:         "1",
		ChannelID:     "2",
		MessageID:     "3",
		ApplicationID: "4",
		Data:          componentInteractionData{ComponentType: 2, CustomID: "approve:42"},
	})
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	s := string(raw)
	for _, want := range []string{`"type":3`, `"custom_id":"approve:42"`, `"component_type":2`} {
		if !strings.Contains(s, want) {
			t.Errorf("payload %s missing %s", s, want)
		}
	}
	if strings.Contains(s, "guild_id") {
		t.Errorf("empty guild ID must be omitted for DMs: %s", s)
	}
}

func TestBotAccountsCannotInvokeComponents(t *testing.T) {
	c := &DiscordClient{meta: &UserLoginMetadata{IsBot: true}}
	if err := c.InvokeComponent(nil, nil, "1", "x", nil); err == nil {
		t.Error("bot account allowed to press a component")
	}
}
