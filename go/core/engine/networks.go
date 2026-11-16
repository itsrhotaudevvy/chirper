package engine

import (
	"fmt"
	"strings"

	"gopkg.in/yaml.v3"
	"maunium.net/go/mautrix/bridgev2"

	fluxer "chirper.dev/connectors/fluxer/pkg/connector"
	"chirper.dev/core/internal/testnet"
	bluesky "go.mau.fi/mautrix-bluesky/pkg/connector"
	gmessages "go.mau.fi/mautrix-gmessages/pkg/connector"
	gvoice "go.mau.fi/mautrix-gvoice/pkg/connector"
	linkedin "go.mau.fi/mautrix-linkedin/pkg/connector"
	meta "go.mau.fi/mautrix-meta/pkg/connector"
	igconnector "go.mau.fi/mautrix-meta/pkg/igconnector"
	slack "go.mau.fi/mautrix-slack/pkg/connector"
	twitter "go.mau.fi/mautrix-twitter/pkg/connector"
	whatsapp "go.mau.fi/mautrix-whatsapp/pkg/connector"
)

// networkSpec is one row of the network registry (ARCHITECTURE.md, bridge IDs).
type networkSpec struct {
	id    string
	color string
	caps  Capabilities
	// newConnector returns a fresh connector. It is nil when the network is not available in this build.
	newConnector func() bridgev2.NetworkConnector
	// name is the display name shown for a network that has no connector to ask for it.
	name string
	// unavailable explains why the network cannot be used in this build. It is empty otherwise.
	unavailable string
	// requires returns why the network cannot be used with the current settings, or "" when it can.
	requires func(Settings) string
	// configure applies the settings to a new connector, before its config is validated.
	configure func(bridgev2.NetworkConnector, Settings)
	// demo networks are only offered when settings.demoNetwork is true.
	demo bool
}

// networkSpecs lists every network. The Signal and Discord rows come from build-tagged files.
func networkSpecs() []networkSpec {
	return []networkSpec{
		{
			id: "whatsapp", color: "#25D366",
			caps:         Capabilities{Edits: true, Reactions: true, Replies: true, Redactions: true, Typing: true, ReadReceipts: true, Stickers: true, Voice: true},
			newConnector: func() bridgev2.NetworkConnector { return &whatsapp.WhatsAppConnector{} },
		},
		telegramSpec(),
		signalSpec(),
		{
			id: "instagram", color: "#E1306C",
			caps:         Capabilities{Edits: true, Reactions: true, Replies: true, Redactions: true, Typing: true, ReadReceipts: true, Stickers: true},
			newConnector: func() bridgev2.NetworkConnector { return &igconnector.IGConnector{} },
		},
		{
			id: "messenger", color: "#FF2542",
			caps:         Capabilities{Edits: true, Reactions: true, Replies: true, Redactions: true, Typing: true, ReadReceipts: true, Stickers: true},
			newConnector: func() bridgev2.NetworkConnector { return &meta.MetaConnector{} },
		},
		{
			id: "twitter", color: "#7C5CFF",
			caps:         Capabilities{Edits: true, Reactions: true, Replies: true, Redactions: true, Typing: true, ReadReceipts: true},
			newConnector: func() bridgev2.NetworkConnector { return &twitter.TwitterConnector{} },
		},
		{
			id: "linkedin", color: "#FF2D7A",
			caps:         Capabilities{Edits: true, Reactions: true, Replies: true, Redactions: true, Typing: true, ReadReceipts: true},
			newConnector: func() bridgev2.NetworkConnector { return &linkedin.LinkedInConnector{} },
		},
		{
			id: "gmessages", color: "#1A73E8",
			caps:         Capabilities{Edits: true, Reactions: true, Replies: true, Redactions: true, Typing: true, ReadReceipts: true, Stickers: true, Voice: true},
			newConnector: func() bridgev2.NetworkConnector { return &gmessages.GMConnector{} },
		},
		{
			id: "gvoice", color: "#34A853",
			caps:         Capabilities{Replies: true, ReadReceipts: true},
			newConnector: func() bridgev2.NetworkConnector { return &gvoice.GVConnector{} },
		},
		{
			id: "slack", color: "#4A154B",
			caps:         Capabilities{Edits: true, Reactions: true, Replies: true, Redactions: true, Typing: true, ReadReceipts: true, Stickers: true, Voice: true},
			newConnector: func() bridgev2.NetworkConnector { return &slack.SlackConnector{} },
		},
		{
			id: "bluesky", color: "#1185FE",
			caps:         Capabilities{Replies: true, ReadReceipts: true},
			newConnector: func() bridgev2.NetworkConnector { return &bluesky.BlueskyConnector{} },
		},
		discordSpec(),
		{
			id: "fluxer", color: "#FF77A9",
			caps:         Capabilities{Edits: true, Reactions: true, Replies: true, Redactions: true, Typing: true, ReadReceipts: true, Stickers: true, Voice: true},
			newConnector: func() bridgev2.NetworkConnector { return fluxer.NewConnector() },
		},
		{
			id: string(testnet.BridgeID), color: "#8E8E93", demo: true,
			caps:         Capabilities{Edits: true, Reactions: true, Replies: true, Redactions: true, Typing: true, ReadReceipts: true, Stickers: true, Voice: true},
			newConnector: func() bridgev2.NetworkConnector { return testnet.New() },
		},
	}
}

// telemetryKeys are top-level example-config keys that are removed before decoding, so the
// connector keeps its zero value (analytics off, no proxy).
var telemetryKeys = map[string]bool{
	"analytics": true, "segment": true, "sentry": true,
	"proxy": true, "get_proxy_url": true, "get_proxy_from": true,
}

// decodeNetworkConfig fills the connector's config struct from its example config. Connector config
// is never read from user files: the example is the only source, and the Chirper overrides apply.
// configure, when not nil, runs after the example config is decoded and before it is validated.
func decodeNetworkConfig(conn bridgev2.NetworkConnector, configure func()) error {
	example, data, _ := conn.GetConfig()
	if data == nil || strings.TrimSpace(example) == "" {
		return nil
	}
	var doc yaml.Node
	if err := yaml.Unmarshal([]byte(example), &doc); err != nil {
		return fmt.Errorf("failed to parse example config: %w", err)
	}
	dropTelemetry(&doc)
	if err := doc.Decode(data); err != nil {
		return fmt.Errorf("failed to decode network config: %w", err)
	}
	if configure != nil {
		configure()
	}
	if v, ok := conn.(bridgev2.ConfigValidatingNetwork); ok {
		return v.ValidateConfig()
	}
	return nil
}

// dropTelemetry removes the telemetryKeys entries of the top-level mapping of doc.
func dropTelemetry(doc *yaml.Node) {
	if len(doc.Content) == 0 || doc.Content[0].Kind != yaml.MappingNode {
		return
	}
	root := doc.Content[0]
	kept := root.Content[:0]
	for i := 0; i+1 < len(root.Content); i += 2 {
		if !telemetryKeys[root.Content[i].Value] {
			kept = append(kept, root.Content[i], root.Content[i+1])
		}
	}
	root.Content = kept
}

// connectorWithConfig returns a connector of spec with its config decoded and configured by s.
func connectorWithConfig(spec networkSpec, s Settings) (bridgev2.NetworkConnector, error) {
	conn := spec.newConnector()
	var configure func()
	if spec.configure != nil {
		configure = func() { spec.configure(conn, s) }
	}
	if err := decodeNetworkConfig(conn, configure); err != nil {
		return nil, err
	}
	return conn, nil
}

// unavailableReason returns why spec cannot be used with settings s, or "" when it can.
func (spec networkSpec) unavailableReason(s Settings) string {
	if spec.unavailable != "" {
		return spec.unavailable
	}
	if spec.requires != nil {
		return spec.requires(s)
	}
	return ""
}
