package engine

import (
	"strconv"

	"maunium.net/go/mautrix/bridgev2"

	telegram "go.mau.fi/mautrix-telegram/pkg/connector"
)

// Build defaults for the Telegram api_id and api_hash. scripts/build-core-aar.sh sets them with
// -ldflags -X from CHIRPER_TG_API_ID and CHIRPER_TG_API_HASH. They are empty in the repository.
var (
	telegramDefaultAPIID   = ""
	telegramDefaultAPIHash = ""
)

// telegramSpec is the Telegram network. It needs an api_id and api_hash, from the settings or the build.
func telegramSpec() networkSpec {
	return networkSpec{
		id: "telegram", color: "#26A5E4", name: "Telegram",
		caps:         Capabilities{Edits: true, Reactions: true, Replies: true, Redactions: true, Typing: true, ReadReceipts: true, Stickers: true, Voice: true},
		newConnector: func() bridgev2.NetworkConnector { return &telegram.TelegramConnector{} },
		requires:     telegramRequirement,
		configure:    applyTelegramCredentials,
	}
}

// telegramRequirement explains the missing credentials, or returns "" when they are set.
func telegramRequirement(s Settings) string {
	if apiID, apiHash := telegramCredentials(s); apiID == 0 || apiHash == "" {
		return "Telegram needs your own api_id and api_hash from my.telegram.org. Add them in Settings, Advanced."
	}
	return ""
}

// applyTelegramCredentials puts the api_id and api_hash into the Telegram connector config.
func applyTelegramCredentials(conn bridgev2.NetworkConnector, s Settings) {
	tc, ok := conn.(*telegram.TelegramConnector)
	if !ok {
		return
	}
	tc.Config.APIID, tc.Config.APIHash = telegramCredentials(s)
}

// telegramCredentials returns the settings' api_id and api_hash, or the build defaults for the
// fields the settings leave empty.
func telegramCredentials(s Settings) (int, string) {
	apiID, apiHash := s.Telegram.APIID, s.Telegram.APIHash
	if apiID == 0 {
		apiID, _ = strconv.Atoi(telegramDefaultAPIID)
	}
	if apiHash == "" {
		apiHash = telegramDefaultAPIHash
	}
	return apiID, apiHash
}
