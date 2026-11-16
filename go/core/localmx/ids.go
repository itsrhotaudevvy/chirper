package localmx

import (
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"strings"

	"github.com/google/uuid"
	"maunium.net/go/mautrix/bridgev2/database"
	"maunium.net/go/mautrix/bridgev2/networkid"
	"maunium.net/go/mautrix/id"

	"chirper.dev/core/store"
)

// hashHex returns the first 16 bytes of the SHA-256 of the parts, hex-encoded. The parts are
// separated by NUL bytes so that different splits never hash the same.
func hashHex(parts ...string) string {
	h := sha256.New()
	for i, p := range parts {
		if i > 0 {
			h.Write([]byte{0})
		}
		h.Write([]byte(p))
	}
	return hex.EncodeToString(h.Sum(nil)[:16])
}

func randomHex(n int) string {
	b := make([]byte, n)
	if _, err := rand.Read(b); err != nil {
		panic(err) // crypto/rand does not fail on supported platforms
	}
	return hex.EncodeToString(b)
}

// botUserID is the bridge bot of a network: @<bridgeID>bot:chirper.local.
func (c *Connector) botUserID() id.UserID {
	return id.NewUserID(string(c.bridgeID)+"bot", store.ServerName)
}

// FormatGhostMXID returns @<bridgeID>_<escaped remote user id>:chirper.local.
func (c *Connector) FormatGhostMXID(userID networkid.UserID) id.UserID {
	return id.NewUserID(string(c.bridgeID)+"_"+id.EncodeUserLocalpart(string(userID)), store.ServerName)
}

// ParseGhostMXID is the inverse of FormatGhostMXID. It only accepts ghosts of this connector's bridge.
func (c *Connector) ParseGhostMXID(userID id.UserID) (networkid.UserID, bool) {
	local, ok := localpartOf(userID)
	if !ok {
		return "", false
	}
	prefix := string(c.bridgeID) + "_"
	encoded := strings.TrimPrefix(local, prefix)
	if !strings.HasPrefix(local, prefix) || encoded == "" {
		return "", false
	}
	decoded, err := id.DecodeUserLocalpart(encoded)
	if err != nil || decoded == "" {
		return "", false
	}
	return networkid.UserID(decoded), true
}

// localpartOf returns the localpart of a user ID on the local server.
func localpartOf(userID id.UserID) (string, bool) {
	suffix := ":" + store.ServerName
	s := string(userID)
	if !strings.HasPrefix(s, "@") || !strings.HasSuffix(s, suffix) {
		return "", false
	}
	local := strings.TrimSuffix(strings.TrimPrefix(s, "@"), suffix)
	return local, local != ""
}

// GenerateDeterministicRoomID returns !<bridgeID>.<hash(portal key)>:chirper.local.
func (c *Connector) GenerateDeterministicRoomID(key networkid.PortalKey) id.RoomID {
	return roomIDFor(string(c.bridgeID) + "." + hashHex(string(key.ID), string(key.Receiver)))
}

// GenerateDeterministicEventID returns $<bridgeID>.<hash(room, message, part)>.
func (c *Connector) GenerateDeterministicEventID(roomID id.RoomID, _ networkid.PortalKey, messageID networkid.MessageID, partID networkid.PartID) id.EventID {
	return id.EventID("$" + string(c.bridgeID) + "." + hashHex(string(roomID), string(messageID), string(partID)))
}

// GenerateReactionEventID returns $<bridgeID>.<hash(room, target, sender, emoji)>.
func (c *Connector) GenerateReactionEventID(roomID id.RoomID, target *database.Message, sender networkid.UserID, emojiID networkid.EmojiID) id.EventID {
	var targetID id.EventID
	if target != nil {
		targetID = target.MXID
	}
	return id.EventID("$" + string(c.bridgeID) + "." + hashHex(string(roomID), string(targetID), string(sender), string(emojiID)))
}

// newLocalEventID returns a fresh $local.<uuid> event ID for events not yet known to the network.
func newLocalEventID() id.EventID {
	return id.EventID("$local." + uuid.NewString())
}

// newRoomID returns a random room ID for rooms created without a deterministic one.
func (c *Connector) newRoomID() id.RoomID {
	return roomIDFor(string(c.bridgeID) + "." + randomHex(16))
}

// roomIDFor builds !<localpart>:chirper.local.
func roomIDFor(localpart string) id.RoomID {
	return id.RoomID("!" + localpart + ":" + store.ServerName)
}

// mediaURI builds mxc://chirper.local/<mediaID>.
func mediaURI(mediaID string) id.ContentURIString {
	return id.ContentURI{Homeserver: store.ServerName, FileID: mediaID}.CUString()
}
