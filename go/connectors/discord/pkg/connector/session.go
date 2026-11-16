// Chirper - Discord bridgev2 network connector.
// Copyright (C) 2026 Chirper contributors
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

package connector

import (
	"time"

	"github.com/bwmarrin/discordgo"

	"chirper.dev/connectors/discord/pkg/httpsec"
)

// apiHTTP serves discordgo REST calls, with timeouts and a body cap.
var apiHTTP = httpsec.NewClient(30*time.Second, httpsec.MaxJSONBytes)

// gatewayHTTP performs the WebSocket upgrade of the discordgo gateway.
var gatewayHTTP = httpsec.NewGatewayClient()

// newSession returns a discordgo session that uses the hardened clients. Its log level stays at
// error so that discordgo never writes request URLs or bodies, which can contain message text, to logs.
func newSession(auth string) (*discordgo.Session, error) {
	sess, err := discordgo.New(auth)
	if err != nil {
		return nil, err
	}
	sess.LogLevel = discordgo.LogError
	sess.Client = apiHTTP
	sess.GatewayHTTPClient = gatewayHTTP
	sess.GatewayDialTimeout = httpsec.GatewayHandshakeTimeout
	return sess, nil
}
