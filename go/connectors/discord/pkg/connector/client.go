// Chirper - Discord bridgev2 network connector.
// Copyright (C) 2026 Chirper contributors
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

package connector

import (
	"context"
	"errors"
	"fmt"
	"runtime/debug"
	"sync"
	"time"

	"github.com/bwmarrin/discordgo"
	websocket "github.com/coder/websocket"
	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/networkid"
	"maunium.net/go/mautrix/bridgev2/status"
)

// Gateway intents for bot accounts. Privileged intents (MessageContent, GuildMembers) must also be
// enabled in the Discord Developer Portal; without them Discord closes the gateway with code 4014.
const botIntents = discordgo.IntentGuilds |
	discordgo.IntentGuildMessages |
	discordgo.IntentGuildMessageReactions |
	discordgo.IntentGuildMessageTyping |
	discordgo.IntentDirectMessages |
	discordgo.IntentDirectMessageReactions |
	discordgo.IntentDirectMessageTyping |
	discordgo.IntentMessageContent |
	discordgo.IntentGuildMembers

const eventQueueSize = 1024

// DiscordClient is the per-login NetworkAPI. It owns one discordgo session.
type DiscordClient struct {
	Main      *DiscordConnector
	UserLogin *bridgev2.UserLogin

	mu         sync.Mutex
	meta       *UserLoginMetadata
	userID     string
	session    *discordgo.Session
	connCancel context.CancelFunc
	// gatewaySessionID is the session ID from READY, used for interaction payloads.
	gatewaySessionID string
	// guildChannelsSynced counts, per server, the channels created eagerly; see reserveGuildChannelSync.
	guildChannelsSynced map[string]int
}

var (
	_ bridgev2.NetworkAPI                    = (*DiscordClient)(nil)
	_ bridgev2.NetworkAPIWithUserID          = (*DiscordClient)(nil)
	_ bridgev2.BackfillingNetworkAPI         = (*DiscordClient)(nil)
	_ bridgev2.EditHandlingNetworkAPI        = (*DiscordClient)(nil)
	_ bridgev2.ReactionHandlingNetworkAPI    = (*DiscordClient)(nil)
	_ bridgev2.RedactionHandlingNetworkAPI   = (*DiscordClient)(nil)
	_ bridgev2.ReadReceiptHandlingNetworkAPI = (*DiscordClient)(nil)
	_ bridgev2.TypingHandlingNetworkAPI      = (*DiscordClient)(nil)
)

// LoadUserLogin creates the NetworkAPI for a stored login. It must not touch the network.
func (dc *DiscordConnector) LoadUserLogin(ctx context.Context, login *bridgev2.UserLogin) error {
	meta, ok := login.Metadata.(*UserLoginMetadata)
	if !ok {
		return fmt.Errorf("unexpected user login metadata type %T", login.Metadata)
	}
	login.Client = &DiscordClient{
		Main:      dc,
		UserLogin: login,
		meta:      meta,
		userID:    string(login.ID),
	}
	return nil
}

func (c *DiscordClient) token() string {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.meta.Token
}

func (c *DiscordClient) isBot() bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.meta.IsBot
}

// currentSession returns the live discordgo session, or ErrNotLoggedIn when disconnected.
func (c *DiscordClient) currentSession() (*discordgo.Session, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.session == nil {
		return nil, bridgev2.ErrNotLoggedIn
	}
	return c.session, nil
}

func (c *DiscordClient) selfUserID() string {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.userID
}

func (c *DiscordClient) setGatewayIdentity(userID, sessionID string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.userID = userID
	c.gatewaySessionID = sessionID
}

func (c *DiscordClient) sendState(state status.BridgeState) {
	c.UserLogin.BridgeState.Send(state)
}

// Connect opens the gateway in the background and reports the outcome through BridgeState.
// After the gateway is open, discordgo reconnects on its own; openWithRetry covers failures of the
// initial open with capped exponential backoff. Auth and intent errors are not retried.
func (c *DiscordClient) Connect(ctx context.Context) {
	if c.token() == "" {
		c.sendState(status.BridgeState{StateEvent: status.StateBadCredentials, Error: "discord-not-logged-in"})
		return
	}
	c.Disconnect()
	connCtx, cancel := context.WithCancel(ctx)
	events := make(chan any, eventQueueSize)
	c.mu.Lock()
	c.connCancel = cancel
	c.mu.Unlock()
	go c.processEvents(connCtx, events)
	go c.openWithRetry(connCtx, events)
}

const (
	initialOpenBackoff = time.Second
	maxOpenBackoff     = time.Minute
)

func (c *DiscordClient) openWithRetry(ctx context.Context, events chan any) {
	backoff := initialOpenBackoff
	for {
		err := c.openSession(ctx, events)
		if err == nil || ctx.Err() != nil {
			return
		}
		c.handleOpenError(err)
		if !isTransientOpenError(err) {
			return
		}
		select {
		case <-ctx.Done():
			return
		case <-time.After(backoff):
		}
		backoff = min(backoff*2, maxOpenBackoff)
	}
}

// openSession creates a discordgo session for the login's token and opens its gateway.
// The session is published as c.session before Open so Disconnect can interrupt it.
func (c *DiscordClient) openSession(ctx context.Context, events chan any) error {
	sess, err := newSession(authorizationHeader(c.token(), c.isBot()))
	if err != nil {
		return fmt.Errorf("failed to create Discord client: %w", err)
	}
	if c.isBot() {
		sess.Identify.Intents = botIntents
	}
	sess.EventHandler = func(raw any) {
		select {
		case events <- raw:
		case <-ctx.Done():
		}
	}
	c.mu.Lock()
	c.session = sess
	c.mu.Unlock()
	if err = sess.Open(); err != nil {
		c.mu.Lock()
		if c.session == sess {
			c.session = nil
		}
		c.mu.Unlock()
		_ = sess.Close()
		return err
	}
	return nil
}

// isTransientOpenError reports whether retrying the gateway open could succeed later.
func isTransientOpenError(err error) bool {
	return !errors.Is(err, discordgo.ErrUnauthorized) &&
		websocket.CloseStatus(err) != 4004 &&
		websocket.CloseStatus(err) != 4014
}

// handleOpenError maps gateway open failures to bridge states without leaking the token.
func (c *DiscordClient) handleOpenError(err error) {
	c.UserLogin.Log.Warn().Err(err).Msg("Failed to open Discord gateway")
	switch {
	case errors.Is(err, discordgo.ErrUnauthorized), websocket.CloseStatus(err) == 4004:
		c.sendState(status.BridgeState{
			StateEvent: status.StateBadCredentials,
			Error:      "discord-invalid-token",
			Message:    "Discord rejected the token. Log in again with a valid token.",
		})
	case websocket.CloseStatus(err) == 4014:
		c.sendState(status.BridgeState{
			StateEvent: status.StateUnknownError,
			Error:      "discord-privileged-intents",
			Message:    "Enable the Message Content and Server Members privileged intents for this bot in the Discord Developer Portal, then reconnect.",
		})
	default:
		c.sendState(status.BridgeState{
			StateEvent: status.StateTransientDisconnect,
			Error:      "discord-connect-failed",
			Message:    "Failed to connect to the Discord gateway; retrying.",
		})
	}
}

func (c *DiscordClient) processEvents(ctx context.Context, events <-chan any) {
	for {
		select {
		case <-ctx.Done():
			return
		case raw := <-events:
			c.safeHandleEvent(raw)
		}
	}
}

func (c *DiscordClient) safeHandleEvent(raw any) {
	defer func() {
		if r := recover(); r != nil {
			c.UserLogin.Log.Error().Any("panic", r).Bytes("stack", debug.Stack()).Msg("Panic in Discord event handler")
		}
	}()
	c.handleEvent(raw)
}

// Disconnect closes the gateway. It is safe to call multiple times.
func (c *DiscordClient) Disconnect() {
	c.mu.Lock()
	sess, cancel := c.session, c.connCancel
	c.session, c.connCancel = nil, nil
	c.mu.Unlock()
	if cancel != nil {
		cancel()
	}
	if sess != nil {
		if err := sess.Close(); err != nil {
			c.UserLogin.Log.Debug().Err(err).Msg("Error closing Discord gateway")
		}
	}
}

func (c *DiscordClient) IsLoggedIn() bool {
	return c.token() != ""
}

// LogoutRemote disconnects and forgets the token. Discord's token revocation endpoint is not called,
// so the token remains valid on Discord's side until it is reset in the Discord client.
func (c *DiscordClient) LogoutRemote(ctx context.Context) {
	c.Disconnect()
	c.mu.Lock()
	c.meta.Token = ""
	c.mu.Unlock()
	if err := c.UserLogin.Save(ctx); err != nil {
		c.UserLogin.Log.Err(err).Msg("Failed to save login after logout")
	}
}

func (c *DiscordClient) IsThisUser(ctx context.Context, userID networkid.UserID) bool {
	return string(userID) == c.selfUserID()
}

func (c *DiscordClient) GetUserID() networkid.UserID {
	return makeUserID(c.selfUserID())
}

func (c *DiscordClient) FillBridgeState(state status.BridgeState) status.BridgeState {
	if state.Info == nil {
		state.Info = make(map[string]any)
	}
	state.Info["discord_user_id"] = c.selfUserID()
	state.Info["is_bot"] = c.isBot()
	return state
}
