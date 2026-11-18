// SPDX-License-Identifier: AGPL-3.0-or-later

package connector

import (
	"context"
	"errors"
	"fmt"
	"sync"

	"github.com/rs/zerolog"
	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/networkid"
	"maunium.net/go/mautrix/bridgev2/status"

	"chirper.dev/connectors/fluxer/pkg/fluxerapi"
)

// fluxerClient is the NetworkAPI for one Fluxer login. It owns one Gateway session and a REST client.
type fluxerClient struct {
	login *bridgev2.UserLogin
	main  *FluxerConnector
	meta  *UserLoginMetadata
	cred  fluxerapi.Credential
	api   *fluxerapi.Client

	mu     sync.Mutex
	gw     *fluxerapi.Gateway
	stop   context.CancelFunc
	guilds map[string]string // channel ID -> guild ID; "" for direct channels
	spaces map[string]bool   // guild IDs that have been bridged as spaces
	// guildProps holds each bridged guild's properties, so that its channels can show its icon.
	guildProps map[string]fluxerapi.Guild
}

var (
	_ bridgev2.NetworkAPI                    = (*fluxerClient)(nil)
	_ bridgev2.NetworkAPIWithUserID          = (*fluxerClient)(nil)
	_ bridgev2.BackfillingNetworkAPI         = (*fluxerClient)(nil)
	_ bridgev2.EditHandlingNetworkAPI        = (*fluxerClient)(nil)
	_ bridgev2.RedactionHandlingNetworkAPI   = (*fluxerClient)(nil)
	_ bridgev2.ReactionHandlingNetworkAPI    = (*fluxerClient)(nil)
	_ bridgev2.TypingHandlingNetworkAPI      = (*fluxerClient)(nil)
	_ bridgev2.ReadReceiptHandlingNetworkAPI = (*fluxerClient)(nil)
)

func newClient(login *bridgev2.UserLogin, main *FluxerConnector, meta *UserLoginMetadata) *fluxerClient {
	cred := fluxerapi.Credential{Token: meta.Token, Bot: meta.Bot}
	return &fluxerClient{
		login:      login,
		main:       main,
		meta:       meta,
		cred:       cred,
		api:        fluxerapi.NewClient(meta.APIBase, cred, main.HTTP),
		guilds:     map[string]string{},
		spaces:     map[string]bool{},
		guildProps: map[string]fluxerapi.Guild{},
	}
}

// Connect starts the Gateway loop in the background. It returns immediately, as bridgev2 requires.
func (c *fluxerClient) Connect(ctx context.Context) {
	c.login.BridgeState.Send(status.BridgeState{StateEvent: status.StateConnecting})
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.stop != nil {
		c.stop()
	}
	runCtx, cancel := context.WithCancel(context.WithoutCancel(ctx))
	c.stop = cancel
	go c.runGateway(runCtx)
}

// Disconnect stops the Gateway loop. It does not revoke the credential.
func (c *fluxerClient) Disconnect() {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.stop != nil {
		c.stop()
		c.stop = nil
	}
}

func (c *fluxerClient) IsLoggedIn() bool {
	return c.meta.Token != ""
}

// LogoutRemote revokes a user session. Bot tokens are kept, because the bot owner manages them.
func (c *fluxerClient) LogoutRemote(ctx context.Context) {
	if !c.cred.Bot {
		if err := c.api.Logout(ctx); err != nil {
			zerolog.Ctx(ctx).Err(err).Msg("Failed to revoke Fluxer session")
		}
	}
	c.Disconnect()
}

func (c *fluxerClient) IsThisUser(ctx context.Context, userID networkid.UserID) bool {
	return string(userID) == c.meta.UserID
}

func (c *fluxerClient) GetUserID() networkid.UserID {
	return makeUserID(c.meta.UserID)
}

// runGateway discovers the Gateway, then keeps a session alive until ctx ends.
func (c *fluxerClient) runGateway(ctx context.Context) {
	log := zerolog.Ctx(ctx).With().Str("component", "fluxer-gateway").Logger()
	ctx = log.WithContext(ctx)

	gwURL, err := c.resolveGatewayURL(ctx)
	if err != nil {
		log.Err(err).Msg("Failed to resolve the Fluxer gateway")
		c.login.BridgeState.Send(status.BridgeState{StateEvent: status.StateUnknownError, Error: "fluxer-discovery-failed"})
		return
	}
	gw := fluxerapi.NewGateway(gwURL, c.cred, c.handleEvent, c.handleGatewayState)
	c.mu.Lock()
	c.gw = gw
	c.mu.Unlock()

	err = gw.Run(ctx)
	if ctx.Err() != nil {
		return
	}
	if errors.Is(err, fluxerapi.ErrGatewayAuth) {
		log.Err(err).Msg("Fluxer rejected the stored credential")
		c.login.BridgeState.Send(status.BridgeState{StateEvent: status.StateBadCredentials, Error: "fluxer-invalid-token"})
		return
	}
	log.Err(err).Msg("Fluxer gateway stopped")
	c.login.BridgeState.Send(status.BridgeState{StateEvent: status.StateUnknownError, Error: "fluxer-gateway-failed"})
}

// resolveGatewayURL refreshes the instance endpoints and returns the Gateway URL. Bots ask the
// API for it (GET /gateway/bot); user sessions use endpoints.gateway from discovery.
func (c *fluxerClient) resolveGatewayURL(ctx context.Context) (string, error) {
	policy := instancePolicy()
	ep, err := fluxerapi.Discover(ctx, c.main.HTTP, c.meta.Instance, policy)
	if err != nil {
		return "", err
	}
	if ep.API != c.meta.APIBase || ep.Media != c.meta.MediaBase {
		c.updateEndpoints(ctx, ep)
	}
	if !c.cred.Bot {
		return ep.Gateway, nil
	}
	gw, err := c.api.GatewayURL(ctx)
	if err != nil {
		return "", err
	}
	if err := policy.CheckURL(gw, "wss", "ws"); err != nil {
		return "", fmt.Errorf("fluxer: gateway URL from the API: %w", err)
	}
	return gw, nil
}

func (c *fluxerClient) updateEndpoints(ctx context.Context, ep *fluxerapi.Endpoints) {
	c.meta.APIBase = ep.API
	c.meta.MediaBase = ep.Media
	c.api = fluxerapi.NewClient(ep.API, c.cred, c.main.HTTP)
	if err := c.login.Save(ctx); err != nil {
		zerolog.Ctx(ctx).Err(err).Msg("Failed to save refreshed instance endpoints")
	}
}

func (c *fluxerClient) handleGatewayState(state fluxerapi.GatewayState, err error) {
	switch state {
	case fluxerapi.GatewayConnected:
		c.login.BridgeState.Send(status.BridgeState{StateEvent: status.StateConnected})
	case fluxerapi.GatewayDisconnected:
		c.login.BridgeState.Send(status.BridgeState{StateEvent: status.StateTransientDisconnect, Error: "fluxer-gateway-disconnected"})
	}
}

// rememberChannel records which guild a channel belongs to, so that events without guild_id
// (such as MESSAGE_ACK) map to the right portal.
func (c *fluxerClient) rememberChannel(channelID, guildID string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.guilds[channelID] = guildID
}

func (c *fluxerClient) rememberSpace(guildID string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.spaces[guildID] = true
}

// rememberGuild keeps a guild's properties, so that the channels inside it can show its icon.
func (c *fluxerClient) rememberGuild(g fluxerapi.Guild) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.guildProps[g.ID] = g
}

// guildChannelAvatar returns the icon of a guild for the channels inside it. It returns nil, which
// leaves the avatar alone, when the guild has not been seen yet in this session.
func (c *fluxerClient) guildChannelAvatar(guildID string) *bridgev2.Avatar {
	c.mu.Lock()
	g, ok := c.guildProps[guildID]
	c.mu.Unlock()
	if !ok {
		return nil
	}
	return c.avatar("/icons/"+g.ID, g.Icon)
}

func (c *fluxerClient) isSpace(id string) bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.spaces[id]
}

// portalKeyFor returns the portal for a channel. Guild channels are shared; DMs are per login.
func (c *fluxerClient) portalKeyFor(channelID, guildID string) networkid.PortalKey {
	if guildID == "" {
		c.mu.Lock()
		guildID = c.guilds[channelID]
		c.mu.Unlock()
	}
	return makePortalKey(channelID, guildID == "", c.login.ID)
}

func (c *fluxerClient) myID() string {
	return c.meta.UserID
}

func (c *fluxerClient) senderOf(authorID string) bridgev2.EventSender {
	return senderFor(c.login.ID, c.myID(), authorID)
}
