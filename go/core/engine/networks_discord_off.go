//go:build nodiscord

package engine

// discordSpec is the Discord network. This build excludes the Discord connector (-tags nodiscord).
func discordSpec() networkSpec {
	return networkSpec{
		id:          "discord",
		color:       "#5865F2",
		name:        "Discord",
		unavailable: "not supported yet: this build excludes the Discord connector",
	}
}
