//go:build !nodiscord

package engine

import (
	"maunium.net/go/mautrix/bridgev2"

	discord "chirper.dev/connectors/discord/pkg/connector"
)

// discordSpec is the Discord network (users and bots).
func discordSpec() networkSpec {
	return networkSpec{
		id: "discord", color: "#5865F2",
		caps:         Capabilities{Edits: true, Reactions: true, Replies: true, Redactions: true, Typing: true, ReadReceipts: true, Stickers: true, Voice: true},
		newConnector: func() bridgev2.NetworkConnector { return &discord.DiscordConnector{} },
	}
}
