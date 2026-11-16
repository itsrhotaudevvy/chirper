//go:build withsignal

package engine

import (
	"maunium.net/go/mautrix/bridgev2"

	signal "go.mau.fi/mautrix-signal/pkg/connector"
)

// signalSpec is the Signal network. It needs cgo and the libsignal static library, so it is only
// compiled for Android (and for desktop builds with the withsignal tag).
func signalSpec() networkSpec {
	return networkSpec{
		id: "signal", color: "#3A76F0",
		caps:         Capabilities{Edits: true, Reactions: true, Replies: true, Redactions: true, Typing: true, ReadReceipts: true, Stickers: true, Voice: true},
		newConnector: func() bridgev2.NetworkConnector { return &signal.SignalConnector{} },
	}
}
