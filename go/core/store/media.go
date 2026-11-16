package store

import (
	"context"
	"fmt"
)

// PutMedia records a media file. The bytes must already be at m.Path.
func (s *Store) PutMedia(ctx context.Context, m *Media) error {
	_, err := s.db.Exec(ctx, `
		INSERT INTO media (media_id, mime, size, file_name, path, encrypted_key_json, created_ts)
		VALUES ($1, $2, $3, $4, $5, $6, $7)`,
		m.MediaID, m.Mime, m.Size, m.FileName, m.Path, m.EncryptedKeyJSON, m.CreatedTS)
	if err != nil {
		return fmt.Errorf("failed to put media: %w", err)
	}
	return nil
}

// GetMedia returns a media record, or ErrNotFound.
func (s *Store) GetMedia(ctx context.Context, mediaID string) (*Media, error) {
	return queryOne(ctx, s, func(row rowScanner) (*Media, error) {
		var m Media
		err := row.Scan(&m.MediaID, &m.Mime, &m.Size, &m.FileName, &m.Path, &m.EncryptedKeyJSON, &m.CreatedTS)
		return &m, err
	}, `SELECT media_id, mime, size, file_name, path, encrypted_key_json, created_ts FROM media WHERE media_id = $1`,
		mediaID)
}
