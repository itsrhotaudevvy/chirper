package localmx

import (
	"context"
	"fmt"
	"io"
	"os"
	"path/filepath"

	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/event"
	"maunium.net/go/mautrix/id"

	"chirper.dev/core/store"
)

// mediaPath resolves a local mxc:// URI to the file holding its bytes. The ID is validated and
// resolved inside the media directory, and the media must be known to the store.
func (c *Connector) mediaPath(ctx context.Context, uri id.ContentURIString, file *event.EncryptedFileInfo) (string, error) {
	if file != nil {
		return "", fmt.Errorf("%w: encrypted media", ErrNotSupported)
	}
	parsed, err := uri.Parse()
	if err != nil {
		return "", err
	}
	if parsed.Homeserver != store.ServerName {
		return "", fmt.Errorf("media %s is not local", uri)
	}
	path, err := ResolveMediaFile(c.mediaDir, parsed.FileID)
	if err != nil {
		return "", err
	}
	if _, err := c.st.GetMedia(ctx, parsed.FileID); err != nil {
		return "", fmt.Errorf("failed to find media: %w", err)
	}
	return path, nil
}

// DownloadMedia implements bridgev2.MatrixAPI.
func (i *intent) DownloadMedia(ctx context.Context, uri id.ContentURIString, file *event.EncryptedFileInfo) ([]byte, error) {
	path, err := i.c.mediaPath(ctx, uri, file)
	if err != nil {
		return nil, err
	}
	return os.ReadFile(path)
}

// DownloadMediaToFile implements bridgev2.MatrixAPI. A writable file is a temporary copy, so the
// stored bytes are never modified.
func (i *intent) DownloadMediaToFile(ctx context.Context, uri id.ContentURIString, file *event.EncryptedFileInfo,
	writable bool, callback func(*os.File) error,
) error {
	path, err := i.c.mediaPath(ctx, uri, file)
	if err != nil {
		return err
	}
	if !writable {
		f, err := os.Open(path)
		if err != nil {
			return err
		}
		defer f.Close()
		return callback(f)
	}
	return i.c.withTempCopy(path, callback)
}

// withTempCopy runs callback on a temporary, writable copy of path, which is removed afterwards.
func (c *Connector) withTempCopy(path string, callback func(*os.File) error) error {
	src, err := os.Open(path)
	if err != nil {
		return err
	}
	defer src.Close()
	tmp, err := os.CreateTemp(c.mediaDir, ".work-*")
	if err != nil {
		return fmt.Errorf("failed to create temp file: %w", err)
	}
	defer os.Remove(tmp.Name())
	defer tmp.Close()
	if _, err := io.Copy(tmp, src); err != nil {
		return fmt.Errorf("failed to copy media: %w", err)
	}
	if _, err := tmp.Seek(0, io.SeekStart); err != nil {
		return err
	}
	return callback(tmp)
}

// newMediaFile creates a new 0600 file for a media ID and returns the ID and path.
func (c *Connector) newMediaFile() (string, string, *os.File, error) {
	mediaID := randomHex(16)
	path := filepath.Join(c.mediaDir, mediaID)
	f, err := os.OpenFile(path, os.O_CREATE|os.O_EXCL|os.O_RDWR, 0o600)
	if err != nil {
		return "", "", nil, fmt.Errorf("failed to create media file: %w", err)
	}
	return mediaID, path, f, nil
}

// UploadMedia implements bridgev2.MatrixAPI. Media is never encrypted for local storage.
func (i *intent) UploadMedia(ctx context.Context, _ id.RoomID, data []byte, fileName, mimeType string) (id.ContentURIString, *event.EncryptedFileInfo, error) {
	mediaID, path, f, err := i.c.newMediaFile()
	if err != nil {
		return "", nil, err
	}
	if _, err := f.Write(data); err != nil {
		_ = f.Close()
		_ = os.Remove(path)
		return "", nil, fmt.Errorf("failed to write media: %w", err)
	}
	if err := f.Close(); err != nil {
		_ = os.Remove(path)
		return "", nil, fmt.Errorf("failed to close media: %w", err)
	}
	err = i.c.st.PutMedia(ctx, &store.Media{
		MediaID:   mediaID,
		Mime:      mimeType,
		Size:      int64(len(data)),
		FileName:  fileName,
		Path:      path,
		CreatedTS: nowMS(),
	})
	if err != nil {
		_ = os.Remove(path)
		return "", nil, err
	}
	return mediaURI(mediaID), nil, nil
}

// UploadMediaStream implements bridgev2.MatrixAPI. The callback writes straight into the media
// file; a replacement file returned by the callback takes its place.
func (i *intent) UploadMediaStream(ctx context.Context, _ id.RoomID, _ int64, _ bool, cb bridgev2.FileStreamCallback,
) (id.ContentURIString, *event.EncryptedFileInfo, error) {
	mediaID, path, f, err := i.c.newMediaFile()
	if err != nil {
		return "", nil, err
	}
	res, err := cb(f)
	closeErr := f.Close()
	if err == nil && closeErr != nil {
		err = closeErr
	}
	if err == nil && res == nil {
		err = fmt.Errorf("media callback returned no result")
	}
	if err != nil {
		_ = os.Remove(path)
		return "", nil, err
	}
	if res.ReplacementFile != "" {
		if err := os.Rename(res.ReplacementFile, path); err != nil {
			_ = os.Remove(path)
			return "", nil, fmt.Errorf("failed to move replacement file: %w", err)
		}
	}
	info, err := os.Stat(path)
	if err != nil {
		_ = os.Remove(path)
		return "", nil, err
	}
	err = i.c.st.PutMedia(ctx, &store.Media{
		MediaID:   mediaID,
		Mime:      res.MimeType,
		Size:      info.Size(),
		FileName:  res.FileName,
		Path:      path,
		CreatedTS: nowMS(),
	})
	if err != nil {
		_ = os.Remove(path)
		return "", nil, err
	}
	return mediaURI(mediaID), nil, nil
}
