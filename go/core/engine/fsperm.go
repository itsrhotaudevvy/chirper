package engine

import (
	"io/fs"
	"os"
	"path/filepath"
)

// Permissions of everything Chirper keeps on disk: directories are owner-only, and so are the
// databases, media, logs and settings. The umask could only make them stricter, so they are set
// explicitly, and SQLite's own -wal and -shm files are covered by the walk.
const (
	privateDirMode  fs.FileMode = 0o700
	privateFileMode fs.FileMode = 0o600
)

// tightenTree sets owner-only permissions on dir and everything under it. Symbolic links are never
// followed or changed, and nothing outside dir is touched.
func tightenTree(dir string) error {
	return filepath.WalkDir(dir, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		switch {
		case d.Type()&fs.ModeSymlink != 0:
			return nil
		case d.IsDir():
			return setMode(path, privateDirMode)
		case d.Type().IsRegular():
			return setMode(path, privateFileMode)
		}
		return nil
	})
}

// setMode changes path to mode only when it differs, so a walk over a large media store does
// not issue a chmod per file on every start.
func setMode(path string, mode fs.FileMode) error {
	info, err := os.Lstat(path)
	if err != nil {
		return err
	}
	if info.Mode().Perm() == mode {
		return nil
	}
	return os.Chmod(path, mode)
}
