package facade

import (
	"encoding/json"
	"fmt"

	"maunium.net/go/mautrix/id"
)

func roomIDOf(roomID string) id.RoomID {
	return id.RoomID(roomID)
}

func eventIDOf(eventID string) id.EventID {
	return id.EventID(eventID)
}

// mustJSON encodes v. Every value passed here is a plain data type, so encoding cannot fail.
func mustJSON(v any) string {
	raw, err := json.Marshal(v)
	if err != nil {
		panic(fmt.Sprintf("openchirp: failed to encode %T: %v", v, err))
	}
	return string(raw)
}
