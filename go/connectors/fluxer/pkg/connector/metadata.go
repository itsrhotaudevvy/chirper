// SPDX-License-Identifier: AGPL-3.0-or-later

package connector

// UserLoginMetadata is stored with each login. It holds the credential, so it must never be logged.
type UserLoginMetadata struct {
	// Instance is the origin the account signed in to, for example https://fluxer.app.
	Instance string `json:"instance"`
	// APIBase is endpoints.api_public from discovery.
	APIBase string `json:"api_base"`
	// MediaBase is endpoints.media from discovery, used for avatars and icons.
	MediaBase string `json:"media_base"`
	// Token is the user session token or the bot token.
	Token string `json:"token"`
	// Bot is true when Token is a bot token.
	Bot bool `json:"bot,omitempty"`
	// UserID is the account's Fluxer snowflake.
	UserID string `json:"user_id"`
}
