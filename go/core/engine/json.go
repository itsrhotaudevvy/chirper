package engine

import (
	"encoding/json"
	"fmt"
)

// mustJSON encodes v as JSON. Every value passed here is a plain struct or map of strings, numbers
// and booleans, so encoding cannot fail; a failure is a programming error and panics.
func mustJSON(v any) string {
	raw, err := json.Marshal(v)
	if err != nil {
		panic(fmt.Sprintf("engine: failed to encode %T: %v", v, err))
	}
	return string(raw)
}
