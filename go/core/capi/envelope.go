package main

import (
	"encoding/json"
)

// okBody and errBody are the two envelope shapes returned by chirper_call.
type okBody struct {
	OK    bool            `json:"ok"`
	Value json.RawMessage `json:"value"`
}

type errBody struct {
	OK    bool   `json:"ok"`
	Error string `json:"error"`
}

// okEnvelope wraps a raw JSON value. A nil value becomes null. An invalid value becomes an error
// envelope, so the output is always valid JSON.
func okEnvelope(value json.RawMessage) []byte {
	if len(value) == 0 {
		value = json.RawMessage("null")
	}
	b, err := json.Marshal(okBody{OK: true, Value: value})
	if err != nil {
		return errorEnvelope(err)
	}
	return b
}

// errorEnvelope wraps err in an error envelope.
func errorEnvelope(err error) []byte {
	b, merr := json.Marshal(errBody{OK: false, Error: err.Error()})
	if merr != nil {
		return []byte(`{"ok":false,"error":"internal error"}`)
	}
	return b
}
