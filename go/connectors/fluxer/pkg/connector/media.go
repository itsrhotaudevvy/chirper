// SPDX-License-Identifier: AGPL-3.0-or-later

package connector

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"

	"maunium.net/go/mautrix/bridgev2"
	"maunium.net/go/mautrix/bridgev2/networkid"

	"chirper.dev/connectors/fluxer/pkg/fluxerapi"
)

// maxMediaBytes bounds one downloaded attachment or image held in memory.
const maxMediaBytes = 100 << 20

// avatar returns an avatar or icon for a hash under the instance's media base. A nil or empty
// hash means "no image", which removes any avatar the portal or ghost has.
func (c *fluxerClient) avatar(path string, hash *string) *bridgev2.Avatar {
	if hash == nil || *hash == "" {
		return &bridgev2.Avatar{Remove: true}
	}
	src := imageURL(c.meta.MediaBase, path, *hash)
	return &bridgev2.Avatar{
		ID: networkid.AvatarID(*hash),
		Get: func(ctx context.Context) ([]byte, error) {
			return downloadBytes(ctx, c.main.HTTP, src, maxMediaBytes)
		},
	}
}

// imageURL builds {base}{path}/{hash}.{ext}. Animated hashes start with "a_" and use gif.
func imageURL(mediaBase, path, hash string) string {
	ext := "webp"
	if strings.HasPrefix(hash, "a_") {
		ext = "gif"
	}
	return fmt.Sprintf("%s%s/%s.%s", mediaBase, path, hash, ext)
}

// downloadBytes fetches a URL and fails when the body is larger than limit.
func downloadBytes(ctx context.Context, hc *http.Client, target string, limit int64) ([]byte, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, target, nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("User-Agent", fluxerapi.UserAgent)
	resp, err := hc.Do(req)
	if err != nil {
		// The transport error repeats the URL, which carries a signature; keep only the cause.
		var urlErr *url.Error
		if errors.As(err, &urlErr) {
			err = urlErr.Err
		}
		return nil, fmt.Errorf("fetching media: %w", err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("fetching media: HTTP %d", resp.StatusCode)
	}
	data, err := io.ReadAll(io.LimitReader(resp.Body, limit+1))
	if err != nil {
		return nil, fmt.Errorf("reading media: %w", err)
	}
	if int64(len(data)) > limit {
		return nil, fmt.Errorf("media is larger than %d bytes", limit)
	}
	return data, nil
}
