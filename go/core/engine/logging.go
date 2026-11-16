package engine

import (
	"io"
	"os"
	"path/filepath"
	"strings"
	"sync"

	"github.com/rs/zerolog"
	"gopkg.in/natefinch/lumberjack.v2"

	"chirper.dev/core/security"
)

// logEnv selects the core log level: debug, info, error. Anything else, including unset, is warn.
const logEnv = "CHIRPER_LOG"

// Log file limits: the current file plus one rotated copy, 5 MB each, owner-only (lumberjack creates
// files with mode 0600).
const (
	logMaxSizeMB = 5
	logBackups   = 1
)

// openLog returns the core logger, which writes to logs/core.log. Every line passes through
// security.RedactingWriter, so sensitive fields such as message bodies, tokens, cookies and phone
// numbers are dropped even at debug level.
func openLog(dataDir string) (zerolog.Logger, io.Closer, error) {
	dir := filepath.Join(dataDir, "logs")
	if err := os.MkdirAll(dir, privateDirMode); err != nil {
		return zerolog.Logger{}, nil, err
	}
	file := &lumberjack.Logger{
		Filename:   filepath.Join(dir, "core.log"),
		MaxSize:    logMaxSizeMB,
		MaxBackups: logBackups,
	}
	closable := &closableWriter{w: file}
	w := security.NewRedactingWriter(closable, dataDir)
	logger := zerolog.New(w).Level(logLevel(os.Getenv(logEnv))).With().Timestamp().Logger()
	return logger, closable, nil
}

// closableWriter stops writing for good once closed. Without this, a log line from a background
// task that outlives Stop would make lumberjack reopen (and recreate) the log file.
type closableWriter struct {
	mu     sync.Mutex
	w      io.WriteCloser
	closed bool
}

// Write implements io.Writer. After Close, writes are discarded.
func (c *closableWriter) Write(p []byte) (int, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.closed {
		return len(p), nil
	}
	return c.w.Write(p)
}

// Close closes the file. It is idempotent.
func (c *closableWriter) Close() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.closed {
		return nil
	}
	c.closed = true
	return c.w.Close()
}

// logLevel maps the CHIRPER_LOG value to a level. The default is warn.
func logLevel(v string) zerolog.Level {
	switch strings.ToLower(strings.TrimSpace(v)) {
	case "debug":
		return zerolog.DebugLevel
	case "info":
		return zerolog.InfoLevel
	case "error":
		return zerolog.ErrorLevel
	}
	return zerolog.WarnLevel
}
