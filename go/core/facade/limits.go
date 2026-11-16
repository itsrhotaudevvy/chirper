package facade

import (
	"encoding/json"
	"errors"
	"fmt"
	"unicode"
)

// maxLen returns an error naming field when value is longer than max bytes.
func maxLen(field, value string, max int) error {
	if len(value) > max {
		return fmt.Errorf("%s is too long (max %d bytes)", field, max)
	}
	return nil
}

// checkAll returns the first non-nil error of errs.
func checkAll(errs ...error) error {
	for _, err := range errs {
		if err != nil {
			return err
		}
	}
	return nil
}

// clampLimit keeps a page size within 1..maxLimit.
func clampLimit(limit int) int {
	return min(max(limit, 1), maxLimit)
}

// parseStringMap decodes a JSON object of strings. The error never quotes the input, because login
// input and cookies are secrets.
func parseStringMap(raw, what string) (map[string]string, error) {
	var out map[string]string
	if err := json.Unmarshal([]byte(raw), &out); err != nil {
		return nil, fmt.Errorf("invalid %s", what)
	}
	return out, nil
}

// parseCookies decodes the cookies of a login step. The payload is capped at maxCookieBytes, and
// entries whose name or value contains a control character are dropped, since no cookie can hold
// one.
func parseCookies(raw string) (map[string]string, error) {
	if len(raw) > maxCookieBytes {
		return nil, errors.New("login cookies are too large")
	}
	in, err := parseStringMap(raw, "login cookies")
	if err != nil {
		return nil, err
	}
	out := make(map[string]string, len(in))
	for name, value := range in {
		if hasControl(name) || hasControl(value) {
			continue
		}
		out[name] = value
	}
	return out, nil
}

// hasControl reports whether s contains a control character.
func hasControl(s string) bool {
	for _, r := range s {
		if unicode.IsControl(r) {
			return true
		}
	}
	return false
}
