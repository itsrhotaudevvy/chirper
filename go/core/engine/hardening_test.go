package engine

import (
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/rs/zerolog"
)

func modeOf(t *testing.T, path string) os.FileMode {
	t.Helper()
	info, err := os.Lstat(path)
	if err != nil {
		t.Fatal(err)
	}
	return info.Mode().Perm()
}

func TestTightenTreeSetsOwnerOnlyModes(t *testing.T) {
	root := t.TempDir()
	outside := filepath.Join(t.TempDir(), "outside.txt")
	if err := os.WriteFile(outside, []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	dir := filepath.Join(root, "data")
	media := filepath.Join(dir, "media")
	if err := os.MkdirAll(media, 0o755); err != nil {
		t.Fatal(err)
	}
	db := filepath.Join(dir, "chirper.db")
	mediaFile := filepath.Join(media, "f")
	for _, f := range []string{db, mediaFile} {
		if err := os.WriteFile(f, []byte("x"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	link := filepath.Join(dir, "link")
	if err := os.Symlink(outside, link); err != nil {
		t.Fatal(err)
	}

	if err := tightenTree(dir); err != nil {
		t.Fatal(err)
	}
	for path, want := range map[string]os.FileMode{
		dir: 0o700, media: 0o700, db: 0o600, mediaFile: 0o600,
	} {
		if got := modeOf(t, path); got != want {
			t.Errorf("%s mode = %o, want %o", path, got, want)
		}
	}
	if got := modeOf(t, outside); got != 0o644 {
		t.Errorf("file behind a symlink changed to %o", got)
	}
}

func TestNewWithOptionsSecuresTheDataDirectory(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "data")
	if err := os.MkdirAll(filepath.Join(dir, "logs"), 0o777); err != nil {
		t.Fatal(err)
	}
	e, err := NewWithOptions(Options{DataDir: dir}, &collector{})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(e.Stop)
	for _, path := range []string{dir, filepath.Join(dir, "media"), filepath.Join(dir, "logs")} {
		if got := modeOf(t, path); got != 0o700 {
			t.Errorf("%s mode = %o, want 700", path, got)
		}
	}
	for _, path := range []string{filepath.Join(dir, "chirper.db"), filepath.Join(dir, "logs", "core.log")} {
		if got := modeOf(t, path); got != 0o600 {
			t.Errorf("%s mode = %o, want 600", path, got)
		}
	}
}

func TestNewWithOptionsRejectsKeysOfTheWrongLength(t *testing.T) {
	for _, n := range []int{1, DBKeyBytes - 1, DBKeyBytes + 1} {
		if _, err := NewWithOptions(Options{DataDir: t.TempDir(), DBKey: make([]byte, n)}, &collector{}); err == nil {
			t.Errorf("key of %d bytes accepted", n)
		}
	}
}

func TestGoSafeReportsPanicsAsCoreErrors(t *testing.T) {
	col := &collector{}
	e, err := NewWithOptions(Options{DataDir: t.TempDir()}, col)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(e.Stop)
	if err := e.Start(t.Context()); err != nil {
		t.Fatal(err)
	}
	e.goSafe(func() { panic("boom") })
	waitFor(t, "core_error for the panic", func() bool {
		for _, ev := range col.snapshot() {
			if strings.HasPrefix(ev, "core_error:") && strings.Contains(ev, "internal error: boom") {
				return true
			}
		}
		return false
	})
}

func TestDispatcherSurvivesAPanickingEvent(t *testing.T) {
	col := &collector{}
	d := newDispatcher(col)
	var mu sync.Mutex
	var failures []error
	d.onFail = func(err error) {
		mu.Lock()
		defer mu.Unlock()
		failures = append(failures, err)
	}
	go d.run()
	defer d.stop()
	d.queue("bad", func() (string, string, bool) { panic("bad event") })
	d.emit("core_error", `{"message":"after"}`)
	waitFor(t, "event after the panic", func() bool {
		return len(col.snapshot()) == 1
	})
	if got := col.snapshot()[0]; got != `core_error:{"message":"after"}` {
		t.Errorf("delivered %q", got)
	}
	mu.Lock()
	defer mu.Unlock()
	if len(failures) != 1 || !strings.Contains(failures[0].Error(), "bad event") {
		t.Errorf("failures = %v", failures)
	}
}

func TestCheckUploadFileRefusesLinksFoldersAndHugeFiles(t *testing.T) {
	dir := t.TempDir()
	regular := filepath.Join(dir, "pic.png")
	if err := os.WriteFile(regular, []byte("png"), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := checkUploadFile(regular); err != nil {
		t.Errorf("regular file refused: %v", err)
	}

	link := filepath.Join(dir, "link.png")
	if err := os.Symlink(regular, link); err != nil {
		t.Fatal(err)
	}
	if _, err := checkUploadFile(link); err == nil {
		t.Error("symbolic link accepted")
	}
	if _, err := checkUploadFile(dir); err == nil {
		t.Error("folder accepted")
	}
	if _, err := checkUploadFile(filepath.Join(dir, "missing")); err == nil {
		t.Error("missing file accepted")
	}
	if _, err := checkUploadFile(filepath.Join(dir, "nul\x00name")); err == nil {
		t.Error("path with NUL accepted")
	}

	huge := filepath.Join(dir, "huge.bin")
	f, err := os.Create(huge)
	if err != nil {
		t.Fatal(err)
	}
	if err := f.Truncate(maxUploadBytes + 1); err != nil {
		t.Fatal(err)
	}
	_ = f.Close()
	if _, err := checkUploadFile(huge); err == nil {
		t.Error("file over 2 GiB accepted")
	}
}

func TestSendMediaRefusesASymlinkBeforeTouchingTheChat(t *testing.T) {
	dir := t.TempDir()
	e, err := NewWithOptions(Options{DataDir: t.TempDir()}, &collector{})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(e.Stop)
	target := filepath.Join(dir, "secret.txt")
	if err := os.WriteFile(target, []byte("secret"), 0o600); err != nil {
		t.Fatal(err)
	}
	link := filepath.Join(dir, "pic.png")
	if err := os.Symlink(target, link); err != nil {
		t.Fatal(err)
	}
	_, err = e.SendMedia("!nope:chirper.local", MediaUpload{FilePath: link, Kind: "file"})
	if err == nil || !strings.Contains(err.Error(), "regular files") {
		t.Fatalf("SendMedia over a symlink: %v", err)
	}
}

func TestSafeMime(t *testing.T) {
	cases := []struct{ declared, file, want string }{
		{"image/png", "a.png", "image/png"},
		{"Image/PNG; charset=binary", "a", "image/png"},
		{"", "photo.jpg", "image/jpeg"},
		{"", "noext", fallbackMime},
		{"text/html", "a.html", fallbackMime},
		{"image/svg+xml", "a.svg", fallbackMime},
		{"application/xml", "a.xml", fallbackMime},
		{"not a mime", "a", fallbackMime},
		{"image/png\r\nX-Injected: 1", "a", fallbackMime},
		{"weird/..%2f", "a", fallbackMime},
		{"application/pdf", "doc.pdf", "application/pdf"},
	}
	for _, c := range cases {
		if got := safeMime(c.declared, c.file); got != c.want {
			t.Errorf("safeMime(%q, %q) = %q, want %q", c.declared, c.file, got, c.want)
		}
	}
}

func TestSafeFileNameNeverKeepsPathsOrControlCharacters(t *testing.T) {
	long := strings.Repeat("n", 400) + ".png"
	cases := []struct{ declared, file, want string }{
		{"holiday.png", "/x/y/z.png", "holiday.png"},
		{"../../etc/passwd", "/x/y.bin", "passwd"},
		{`..\..\windows\evil.exe`, "/x/y.bin", "evil.exe"},
		{"/etc/shadow", "/x/y.bin", "shadow"},
		{"a\x00b\x1bc.png", "/x/y.bin", "abc.png"},
		{"..", "/x/report.pdf", "report.pdf"},
		{"", "/x/report.pdf", "report.pdf"},
		{"   ", "/x/report.pdf", "report.pdf"},
		{"/", "/x/report.pdf", "report.pdf"},
	}
	for _, c := range cases {
		if got := safeFileName(c.declared, c.file); got != c.want {
			t.Errorf("safeFileName(%q, %q) = %q, want %q", c.declared, c.file, got, c.want)
		}
	}
	if got := safeFileName(long, "/x/y.bin"); len([]rune(got)) != maxFileNameRunes {
		t.Errorf("long name has %d runes, want %d", len([]rune(got)), maxFileNameRunes)
	}
}

func TestMediaFileRejectsTraversalURIs(t *testing.T) {
	e, err := NewWithOptions(Options{DataDir: t.TempDir()}, &collector{})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(e.Stop)
	for _, mxc := range []string{
		"mxc://chirper.local/../../etc/passwd",
		"mxc://chirper.local/" + strings.Repeat("a", 31),
		"mxc://chirper.local/%2e%2e%2fsecret",
		"mxc://chirper.local/0123456789abcdef0123456789abcdeF",
		"mxc://example.org/0123456789abcdef0123456789abcdef",
	} {
		if path, err := e.MediaFile(mxc); err == nil {
			t.Errorf("MediaFile(%q) = %q, want an error", mxc, path)
		}
	}
}

func TestLogLevelFromEnvironment(t *testing.T) {
	cases := map[string]zerolog.Level{
		"":        zerolog.WarnLevel,
		"debug":   zerolog.DebugLevel,
		" DEBUG ": zerolog.DebugLevel,
		"info":    zerolog.InfoLevel,
		"error":   zerolog.ErrorLevel,
		"trace":   zerolog.WarnLevel,
	}
	for in, want := range cases {
		if got := logLevel(in); got != want {
			t.Errorf("logLevel(%q) = %v, want %v", in, got, want)
		}
	}
}

func waitFor(t *testing.T, what string, ok func() bool) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		if ok() {
			return
		}
		time.Sleep(5 * time.Millisecond)
	}
	t.Fatalf("timed out waiting for %s", what)
}
