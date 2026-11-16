package localmx

import (
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"regexp"
)

// mediaIDPattern matches the IDs newMediaFile creates: 16 random bytes, lowercase hex. Anything
// else, including separators, dots and NUL, is refused before it reaches the file system.
var mediaIDPattern = regexp.MustCompile(`^[0-9a-f]{32}$`)

// ResolveMediaFile returns the path of the media file mediaID inside mediaDir. The ID must have
// the exact form newMediaFile creates, so the path can only name a file directly in mediaDir. The
// entry must be a regular file: a symbolic link is refused and never followed.
func ResolveMediaFile(mediaDir, mediaID string) (string, error) {
	if !mediaIDPattern.MatchString(mediaID) {
		return "", errors.New("invalid media id")
	}
	dir := filepath.Clean(mediaDir)
	path := filepath.Join(dir, mediaID)
	if filepath.Dir(path) != dir {
		return "", errors.New("media path escapes the media directory")
	}
	info, err := os.Lstat(path)
	switch {
	case errors.Is(err, fs.ErrNotExist):
		return "", errors.New("media file is missing")
	case err != nil:
		return "", fmt.Errorf("media file is unreadable: %w", errors.Unwrap(err))
	case !info.Mode().IsRegular():
		return "", errors.New("media is not a regular file")
	}
	return path, nil
}
