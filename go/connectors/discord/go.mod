module chirper.dev/connectors/discord

go 1.26.0

replace github.com/bwmarrin/discordgo => github.com/beeper/discordgo v0.0.0-20261104162414-49256f3e1ca7

replace maunium.net/go/mautrix => ../../../third_party/mautrix-go

require (
	github.com/bwmarrin/discordgo v0.29.0
	github.com/coder/websocket v1.8.15
	github.com/gorilla/websocket v1.5.3
	github.com/yuin/goldmark v1.8.7
	go.mau.fi/util v0.11.1
	maunium.net/go/mautrix v0.33.0
)

require (
	filippo.io/edwards25519 v1.2.0 // indirect
	github.com/coreos/go-systemd/v22 v22.7.0 // indirect
	github.com/google/uuid v1.6.0 // indirect
	github.com/mattn/go-colorable v0.1.16 // indirect
	github.com/mattn/go-isatty v0.0.24 // indirect
	github.com/petermattis/goid v0.0.0-20261109124353-7cfa343ad0ab // indirect
	github.com/rs/xid v1.6.0 // indirect
	github.com/rs/zerolog v1.35.1 // indirect
	github.com/tidwall/gjson v1.20.4 // indirect
	github.com/tidwall/match v1.2.0 // indirect
	github.com/tidwall/pretty v1.3.0 // indirect
	github.com/tidwall/sjson v1.2.10 // indirect
	go.mau.fi/zeroconfig v0.2.0 // indirect
	golang.org/x/crypto v0.59.0 // indirect
	golang.org/x/exp v0.0.0-20261117202309-7a49493bfd90 // indirect
	golang.org/x/net v0.63.0 // indirect
	golang.org/x/sys v0.50.0 // indirect
	golang.org/x/text v0.44.0 // indirect
	gopkg.in/natefinch/lumberjack.v2 v2.2.1 // indirect
	gopkg.in/yaml.v3 v3.0.1 // indirect
)
