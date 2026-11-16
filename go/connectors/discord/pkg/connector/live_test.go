// Chirper - Discord bridgev2 network connector.
// Copyright (C) 2026 Chirper contributors
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

package connector

import (
	"os"
	"testing"
)

// TestLiveBotToken verifies a real bot token against Discord. It runs only when
// DISCORD_TEST_BOT_TOKEN is set, so the default test run never touches the network.
func TestLiveBotToken(t *testing.T) {
	token := os.Getenv("DISCORD_TEST_BOT_TOKEN")
	if token == "" {
		t.Skip("DISCORD_TEST_BOT_TOKEN not set")
	}
	me, err := validateToken(token, true)
	if err != nil {
		t.Fatalf("validateToken: %v", err)
	}
	if !me.Bot {
		t.Fatal("token is not a bot account")
	}
	if me.ID == "" {
		t.Fatal("Discord returned no user ID")
	}
}
