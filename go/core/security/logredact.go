package security

import (
	"bytes"
	"encoding/json"
	"io"
	"strings"
	"unicode"
)

// sensitiveWords mark a log field whose value must never be written. A field is redacted when one
// of the words in its name (split at non-letters, so "event_text" and "eventText" both count) is in
// this set, or when the name without separators is.
var sensitiveWords = map[string]bool{
	"body": true, "text": true, "token": true, "cookie": true, "cookies": true, "password": true,
	"phone": true, "email": true, "key": true, "secret": true, "authorization": true,
	"displayname": true,
}

// redactedValue replaces every redacted value.
const redactedValue = "[redacted]"

// RedactingWriter is an io.Writer for zerolog. Each write is one JSON log line; the writer drops
// the value of every sensitive field, replaces private directory paths with "<data>" in string
// values, and writes a placeholder for any line it cannot parse, so a log line never leaks even
// when a caller logs something unexpected.
type RedactingWriter struct {
	w       io.Writer
	private string
}

// NewRedactingWriter wraps w. privateDir is the data directory; paths under it are shortened.
func NewRedactingWriter(w io.Writer, privateDir string) *RedactingWriter {
	return &RedactingWriter{w: w, private: privateDir}
}

// Write implements io.Writer. It always reports the whole input as written.
func (r *RedactingWriter) Write(p []byte) (int, error) {
	line, err := redactLine(p, r.private)
	if err != nil {
		line = []byte(`{"level":"error","message":"log line dropped: not redactable"}` + "\n")
	}
	if _, err := r.w.Write(line); err != nil {
		return 0, err
	}
	return len(p), nil
}

// redactLine returns line with sensitive fields redacted, terminated by a newline.
func redactLine(line []byte, private string) ([]byte, error) {
	dec := json.NewDecoder(bytes.NewReader(line))
	dec.UseNumber()
	var obj map[string]any
	if err := dec.Decode(&obj); err != nil {
		return nil, err
	}
	for k, v := range obj {
		obj[k] = redactValue(k, v, private)
	}
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(obj); err != nil {
		return nil, err
	}
	return buf.Bytes(), nil
}

// redactValue returns v with sensitive values removed, recursively.
func redactValue(key string, v any, private string) any {
	if IsSensitiveField(key) {
		return redactedValue
	}
	switch t := v.(type) {
	case string:
		if private != "" {
			return strings.ReplaceAll(t, private, "<data>")
		}
	case map[string]any:
		for k, x := range t {
			t[k] = redactValue(k, x, private)
		}
	case []any:
		for i, x := range t {
			t[i] = redactValue("", x, private)
		}
	}
	return v
}

// IsSensitiveField reports whether a log field called name carries secret or private content.
func IsSensitiveField(name string) bool {
	words := fieldWords(name)
	for _, w := range words {
		if sensitiveWords[w] {
			return true
		}
	}
	return sensitiveWords[strings.Join(words, "")]
}

// fieldWords splits a field name into lowercase words at separators and camelCase boundaries.
func fieldWords(name string) []string {
	var words []string
	var cur []rune
	var prev rune
	flush := func() {
		if len(cur) > 0 {
			words = append(words, strings.ToLower(string(cur)))
			cur = cur[:0]
		}
	}
	for _, r := range name {
		switch {
		case !unicode.IsLetter(r):
			flush()
		case unicode.IsUpper(r) && unicode.IsLower(prev):
			flush()
			cur = append(cur, r)
		default:
			cur = append(cur, r)
		}
		prev = r
	}
	flush()
	return words
}
