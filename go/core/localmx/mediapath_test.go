package localmx

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestResolveMediaFileAcceptsOnlyGeneratedIDs(t *testing.T) {
	dir := t.TempDir()
	id := strings.Repeat("ab", 16)
	path := filepath.Join(dir, id)
	if err := os.WriteFile(path, []byte("bytes"), 0o600); err != nil {
		t.Fatal(err)
	}
	got, err := ResolveMediaFile(dir, id)
	if err != nil {
		t.Fatalf("valid id refused: %v", err)
	}
	if got != path {
		t.Errorf("path = %q, want %q", got, path)
	}
}

func TestResolveMediaFileRejectsTraversalAndOddIDs(t *testing.T) {
	root := t.TempDir()
	dir := filepath.Join(root, "media")
	if err := os.Mkdir(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	outside := filepath.Join(root, "outside-secret")
	if err := os.WriteFile(outside, []byte("secret"), 0o600); err != nil {
		t.Fatal(err)
	}

	attempts := []string{
		"../" + filepath.Base(outside),
		"..",
		".",
		"/etc/passwd",
		outside,
		"a/../../" + filepath.Base(outside),
		"nul\x00" + strings.Repeat("a", 27),
		"",
		strings.Repeat("a", 31),
		strings.Repeat("a", 33),
		strings.Repeat("A", 32),
		strings.Repeat("g", 32),
		strings.Repeat("ab", 16) + "/x",
		"%2e%2e%2f" + filepath.Base(outside),
	}
	for _, id := range attempts {
		if path, err := ResolveMediaFile(dir, id); err == nil {
			t.Errorf("ResolveMediaFile(%q) = %q, want an error", id, path)
		}
	}
}

func TestResolveMediaFileRefusesSymlinks(t *testing.T) {
	dir := t.TempDir()
	outside := filepath.Join(t.TempDir(), "secret")
	if err := os.WriteFile(outside, []byte("secret"), 0o600); err != nil {
		t.Fatal(err)
	}
	id := strings.Repeat("cd", 16)
	if err := os.Symlink(outside, filepath.Join(dir, id)); err != nil {
		t.Fatal(err)
	}
	if path, err := ResolveMediaFile(dir, id); err == nil {
		t.Errorf("symlink resolved to %q", path)
	}
}

func TestResolveMediaFileRefusesMissingFilesAndFolders(t *testing.T) {
	dir := t.TempDir()
	missing := strings.Repeat("01", 16)
	if _, err := ResolveMediaFile(dir, missing); err == nil {
		t.Error("missing file resolved")
	}
	folder := strings.Repeat("02", 16)
	if err := os.Mkdir(filepath.Join(dir, folder), 0o700); err != nil {
		t.Fatal(err)
	}
	if _, err := ResolveMediaFile(dir, folder); err == nil {
		t.Error("folder resolved")
	}
}
