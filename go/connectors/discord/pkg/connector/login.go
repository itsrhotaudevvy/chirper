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
	"strings"

	"github.com/bwmarrin/discordgo"
	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/database"
)

const (
	LoginFlowIDToken = "token"
	LoginFlowIDBot   = "bot"
	LoginFlowIDQR    = "qr"

	LoginStepIDToken    = "fi.chirper.discord.login.enter_token"
	LoginStepIDBotToken = "fi.chirper.discord.login.enter_bot_token"
	LoginStepIDQR       = "fi.chirper.discord.login.qr"
	LoginStepIDComplete = "fi.chirper.discord.login.complete"

	loginTokenField = "token"
)

// ErrUnknownLoginFlow is returned by CreateLogin for flow IDs the connector does not offer.
var ErrUnknownLoginFlow = errors.New("unknown Discord login flow")

func (dc *DiscordConnector) GetLoginFlows() []bridgev2.LoginFlow {
	return []bridgev2.LoginFlow{{
		ID:          LoginFlowIDBrowser,
		Name:        "Browser",
		Description: "Sign in with your email and password (or passkey) on Discord's own login page.",
	}, {
		ID:          LoginFlowIDQR,
		Name:        "QR code",
		Description: "Scan a QR code with the Discord mobile app to log in to your user account.",
	}, {
		ID:          LoginFlowIDToken,
		Name:        "User token",
		Description: "Log in to your Discord user account with an access token.",
	}, {
		ID:          LoginFlowIDBot,
		Name:        "Bot token",
		Description: "Log in as a Discord bot account with its bot token.",
	}}
}

func (dc *DiscordConnector) CreateLogin(ctx context.Context, user *bridgev2.User, flowID string) (bridgev2.LoginProcess, error) {
	switch flowID {
	case LoginFlowIDToken:
		return &tokenLogin{connector: dc, user: user, isBot: false}, nil
	case LoginFlowIDBot:
		return &tokenLogin{connector: dc, user: user, isBot: true}, nil
	case LoginFlowIDQR:
		return newQRLogin(dc, user), nil
	case LoginFlowIDBrowser:
		return newBrowserLogin(dc, user), nil
	default:
		return nil, fmt.Errorf("%w: %q", ErrUnknownLoginFlow, flowID)
	}
}

// tokenLogin handles both the user-token and bot-token flows, which differ only in the
// prefix used for the Authorization header and the account type they must match.
type tokenLogin struct {
	connector *DiscordConnector
	user      *bridgev2.User
	isBot     bool
}

var _ bridgev2.LoginProcessUserInput = (*tokenLogin)(nil)

func (t *tokenLogin) Start(ctx context.Context) (*bridgev2.LoginStep, error) {
	field := bridgev2.LoginInputDataField{
		Type:        bridgev2.LoginInputFieldTypeToken,
		ID:          loginTokenField,
		Name:        "User token",
		Description: "Your Discord user access token.",
	}
	stepID := LoginStepIDToken
	if t.isBot {
		field.Name = "Bot token"
		field.Description = "The token of your Discord bot application."
		stepID = LoginStepIDBotToken
	}
	return &bridgev2.LoginStep{
		Type:   bridgev2.LoginStepTypeUserInput,
		StepID: stepID,
		UserInputParams: &bridgev2.LoginUserInputParams{
			Fields: []bridgev2.LoginInputDataField{field},
		},
	}, nil
}

func (t *tokenLogin) Cancel() {}

func (t *tokenLogin) SubmitUserInput(ctx context.Context, input map[string]string) (*bridgev2.LoginStep, error) {
	token := strings.TrimSpace(input[loginTokenField])
	if token == "" {
		return nil, errors.New("token is required")
	}
	// Users often paste the "Bot " prefix that the Developer Portal shows; the prefix is re-added internally.
	token = strings.TrimPrefix(token, "Bot ")
	return t.connector.finishLogin(ctx, t.user, token, t.isBot)
}

// authorizationHeader returns the value discordgo expects for the given token.
func authorizationHeader(token string, isBot bool) string {
	if isBot {
		return "Bot " + token
	}
	return token
}

// validateToken checks the token against GET /users/@me and returns the account it belongs to.
// The returned error never contains the token.
func validateToken(token string, isBot bool) (*discordgo.User, error) {
	sess, err := newSession(authorizationHeader(token, isBot))
	if err != nil {
		return nil, fmt.Errorf("failed to create Discord client: %w", err)
	}
	me, err := sess.User("@me")
	if err != nil {
		var restErr *discordgo.RESTError
		if errors.As(err, &restErr) && restErr.Response != nil && restErr.Response.StatusCode == 401 {
			return nil, errors.New("Discord rejected the token; check that it is correct and still valid")
		}
		return nil, fmt.Errorf("failed to verify token with Discord: %w", err)
	}
	return me, nil
}

// finishLogin validates a token, stores it as a new login and starts connecting in the background.
func (dc *DiscordConnector) finishLogin(ctx context.Context, user *bridgev2.User, token string, isBot bool) (*bridgev2.LoginStep, error) {
	me, err := validateToken(token, isBot)
	if err != nil {
		return nil, err
	}
	if me.Bot != isBot {
		if isBot {
			return nil, errors.New("this token belongs to a user account; log in with the user token flow instead")
		}
		return nil, errors.New("this token belongs to a bot account; log in with the bot token flow instead")
	}
	ul, err := user.NewLogin(ctx, &database.UserLogin{
		ID:         makeUserLoginID(me.ID),
		RemoteName: me.Username,
		Metadata:   &UserLoginMetadata{Token: token, IsBot: isBot},
	}, &bridgev2.NewLoginParams{DeleteOnConflict: true})
	if err != nil {
		return nil, err
	}
	client := ul.Client.(*DiscordClient)
	go client.Connect(ul.Log.WithContext(context.Background()))
	return completeLoginStep(ul, fmt.Sprintf("Successfully logged in as %s", me.Username)), nil
}

func completeLoginStep(ul *bridgev2.UserLogin, instructions string) *bridgev2.LoginStep {
	return &bridgev2.LoginStep{
		Type:         bridgev2.LoginStepTypeComplete,
		StepID:       LoginStepIDComplete,
		Instructions: instructions,
		CompleteParams: &bridgev2.LoginCompleteParams{
			UserLoginID: ul.ID,
			UserLogin:   ul,
		},
	}
}
