// SPDX-License-Identifier: AGPL-3.0-or-later

// Package fluxerapi is a small client for the Fluxer HTTP API and Gateway.
package fluxerapi

// Credential is the token that authenticates a client. Bot tokens are sent with
// the "Bot " prefix on HTTP requests; user session tokens are sent as-is.
type Credential struct {
	Token string
	Bot   bool
}

// authorization returns the Authorization header value for HTTP requests.
func (c Credential) authorization() string {
	if c.Bot {
		return "Bot " + c.Token
	}
	return c.Token
}

// String hides the token so that a Credential is safe to pass to log formatters.
func (c Credential) String() string {
	if c.Bot {
		return "Credential{bot, token:REDACTED}"
	}
	return "Credential{user, token:REDACTED}"
}
