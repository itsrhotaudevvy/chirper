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
	"strings"

	"maunium.net/go/mautrix/bridgev2"
)

const (
	LoginFlowIDBrowser = "browser"
	LoginStepIDBrowser = "fi.chirper.discord.login.browser"
)

// discordTokenExtractJS reads the token the Discord web app stores after a successful sign-in.
// It must stay synchronous: the client evaluates it with a plain script evaluation, not a promise.
const discordTokenExtractJS = `(function(){
  try {
    var raw = localStorage.getItem('token');
    if (!raw) return null;
    var value = raw;
    try { value = JSON.parse(raw); } catch (e) {}
    if (typeof value !== 'string' || value.length < 20) return null;
    return value;
  } catch (e) { return null; }
})()`

// browserLogin drives Discord's normal web sign-in page in the app's login web view and takes the
// user token the web app keeps in local storage. This is the flow people expect from a chat client:
// username, password, 2FA and captcha all happen on Discord's own page.
type browserLogin struct {
	connector *DiscordConnector
	user      *bridgev2.User
}

func newBrowserLogin(dc *DiscordConnector, user *bridgev2.User) *browserLogin {
	return &browserLogin{connector: dc, user: user}
}

func (b *browserLogin) Start(ctx context.Context) (*bridgev2.LoginStep, error) {
	return &bridgev2.LoginStep{
		Type:         bridgev2.LoginStepTypeCookies,
		StepID:       LoginStepIDBrowser,
		Instructions: "Sign in to Discord. The window closes by itself as soon as you are logged in.",
		CookiesParams: &bridgev2.LoginCookiesParams{
			URL:       "https://discord.com/login",
			UserAgent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
			Fields: []bridgev2.LoginCookieField{{
				ID:       loginTokenField,
				Required: true,
				Sources: []bridgev2.LoginCookieFieldSource{{
					Type: bridgev2.LoginCookieTypeSpecial,
				}},
			}},
			ExtractJS:         discordTokenExtractJS,
			WaitForURLPattern: `discord\.com/channels`,
			JSRequired:        true,
		},
	}, nil
}

func (b *browserLogin) SubmitCookies(ctx context.Context, cookies map[string]string) (*bridgev2.LoginStep, error) {
	token := strings.Trim(strings.TrimSpace(cookies[loginTokenField]), `"`)
	if token == "" {
		token = strings.Trim(strings.TrimSpace(cookies["authorization"]), `"`)
	}
	if token == "" {
		return nil, errors.New("please provide a valid Discord token to complete sign-in")
	}
	return b.connector.finishLogin(ctx, b.user, token, false)
}

func (b *browserLogin) Cancel() {}
