// SPDX-License-Identifier: AGPL-3.0-or-later

package fluxerapi

import (
	"context"
	"os"
	"testing"
	"time"
)

// TestLiveSmoke runs against a real instance. It is skipped unless FLUXER_TEST_TOKEN is set.
// FLUXER_TEST_INSTANCE selects the origin (default: the official instance), and
// FLUXER_TEST_BOT=1 treats the token as a bot token. The token is never printed.
func TestLiveSmoke(t *testing.T) {
	token := os.Getenv("FLUXER_TEST_TOKEN")
	if token == "" {
		t.Skip("FLUXER_TEST_TOKEN not set")
	}
	instance := os.Getenv("FLUXER_TEST_INSTANCE")
	if instance == "" {
		instance = DefaultInstance
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	ep, err := Discover(ctx, NewHTTPClient(15*time.Second), instance, Policy{AllowInsecure: os.Getenv("FLUXER_TEST_INSECURE") == "1"})
	if err != nil {
		t.Fatalf("discover: %v", err)
	}
	cred := Credential{Token: token, Bot: os.Getenv("FLUXER_TEST_BOT") == "1"}
	c := NewClient(ep.API, cred, nil)
	me, err := c.Me(ctx)
	if err != nil {
		t.Fatalf("me: %v", err)
	}
	if _, err := c.Guilds(ctx); err != nil {
		t.Fatalf("guilds: %v", err)
	}
	t.Logf("authenticated as user %s (bot=%v)", me.ID, me.Bot)
}
