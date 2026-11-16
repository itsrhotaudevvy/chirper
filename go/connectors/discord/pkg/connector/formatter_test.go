// Chirper - Discord bridgev2 network connector.
// Copyright (C) 2026 Chirper contributors
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

package connector

import (
	"strings"
	"testing"

	"maunium.net/go/mautrix/bridgev2/networkid"
	"maunium.net/go/mautrix/event"
	"maunium.net/go/mautrix/id"
)

// fakeGhosts implements ghostMapper with the same "discord_<id>" shape the bridge uses,
// so the tests do not depend on a running bridge.
type fakeGhosts struct{}

func (fakeGhosts) FormatGhostMXID(userID networkid.UserID) id.UserID {
	return id.NewUserID("discord_"+string(userID), "chirper.local")
}

func (fakeGhosts) ParseGhostMXID(mxid id.UserID) (networkid.UserID, bool) {
	localpart, domain, err := mxid.Parse()
	if err != nil || domain != "chirper.local" {
		return "", false
	}
	userID, found := strings.CutPrefix(localpart, "discord_")
	if !found || !isSnowflake(userID) {
		return "", false
	}
	return networkid.UserID(userID), true
}

func isSnowflake(s string) bool {
	if s == "" {
		return false
	}
	for _, r := range s {
		if r < '0' || r > '9' {
			return false
		}
	}
	return true
}

var testGhosts ghostMapper = fakeGhosts{}

func TestDiscordToHTML(t *testing.T) {
	tests := []struct {
		name    string
		input   string
		want    string
		mention []string
	}{
		{name: "bold", input: "**bold**", want: "<strong>bold</strong>"},
		{name: "italic", input: "*it*", want: "<em>it</em>"},
		{name: "underline", input: "__u__", want: "<u>u</u>"},
		{name: "strikethrough", input: "~~s~~", want: "<del>s</del>"},
		{name: "spoiler", input: "||secret||", want: `<span data-mx-spoiler>secret</span>`},
		{
			name:    "user mention",
			input:   "hi <@123>",
			want:    `hi <a href="https://matrix.to/#/@discord_123:chirper.local">@Alice</a>`,
			mention: []string{"123"},
		},
		{name: "custom emoji", input: "<:wave:999>", want: ":wave:"},
		{name: "animated custom emoji", input: "<a:spin:999>", want: ":spin:"},
	}
	names := mentionNames{"123": "Alice"}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, mentioned := discordToHTML(tt.input, names, testGhosts)
			if got != tt.want {
				t.Errorf("discordToHTML(%q) = %q, want %q", tt.input, got, tt.want)
			}
			if len(mentioned) != len(tt.mention) {
				t.Errorf("mentioned = %v, want %v", mentioned, tt.mention)
			}
		})
	}
}

// TestMarkdownRoundTrip checks Discord markdown -> HTML -> Discord markdown.
func TestMarkdownRoundTrip(t *testing.T) {
	tests := []struct {
		name  string
		input string
	}{
		{name: "formatting", input: "**bold** *it* __u__ ~~s~~ ||sp||"},
		{name: "inline code", input: "use `go test`"},
		{name: "mention", input: "hey <@123>"},
	}
	names := mentionNames{"123": "Alice"}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			html, _ := discordToHTML(tt.input, names, testGhosts)
			back, _ := matrixHTMLToDiscord(testGhosts, &event.MessageEventContent{
				MsgType:       event.MsgText,
				Format:        event.FormatHTML,
				FormattedBody: html,
			})
			if back != tt.input {
				t.Errorf("round trip of %q gave %q (html %q)", tt.input, back, html)
			}
		})
	}
}

func TestMatrixToDiscordEscapesPlainText(t *testing.T) {
	got, _ := matrixHTMLToDiscord(testGhosts, &event.MessageEventContent{MsgType: event.MsgText, Body: "1 * 2_3 <x>"})
	want := `1 \* 2\_3 \<x>`
	if got != want {
		t.Errorf("plain text escaped to %q, want %q", got, want)
	}
}

func TestMatrixToDiscordKeepsURLs(t *testing.T) {
	got, _ := matrixHTMLToDiscord(testGhosts, &event.MessageEventContent{MsgType: event.MsgText, Body: "see https://example.com/a_b_c"})
	want := "see https://example.com/a_b_c"
	if got != want {
		t.Errorf("URL changed to %q, want %q", got, want)
	}
}

func TestMatrixMentionOnlyPingsGhosts(t *testing.T) {
	html := `<a href="https://matrix.to/#/@discord_456:chirper.local">Bob</a> and <a href="https://matrix.to/#/@someone:example.org">Carol</a>`
	got, mentions := matrixHTMLToDiscord(testGhosts, &event.MessageEventContent{Format: event.FormatHTML, FormattedBody: html, Body: "x"})
	if got != "<@456> and Carol" {
		t.Errorf("got %q", got)
	}
	if len(mentions) != 1 || mentions[0] != "456" {
		t.Errorf("mentions = %v, want [456]", mentions)
	}
}

func TestDiscordIDFromGhostMXID(t *testing.T) {
	mxid := testGhosts.FormatGhostMXID("42")
	userID, ok := testGhosts.ParseGhostMXID(mxid)
	if !ok || userID != "42" {
		t.Fatalf("round trip of %s = %q, %v", mxid, userID, ok)
	}
	for _, bad := range []string{"@someone:chirper.local", "@discord_abc:chirper.local"} {
		if _, ok := testGhosts.ParseGhostMXID(id.UserID(bad)); ok {
			t.Errorf("%s accepted as a ghost", bad)
		}
	}
}
