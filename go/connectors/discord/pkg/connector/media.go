// Chirper - Discord bridgev2 network connector.
// Copyright (C) 2026 Chirper contributors
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

package connector

import (
	"context"
	"fmt"
	"io"
	"net/http"

	"chirper.dev/connectors/discord/pkg/httpsec"
)

// download fetches a Discord CDN URL, refusing bodies larger than the configured maximum file size.
// It returns the bytes and the server-reported content type.
func (dc *DiscordConnector) download(ctx context.Context, url string) ([]byte, string, error) {
	if err := httpsec.RequireHTTPS(url); err != nil {
		return nil, "", err
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return nil, "", err
	}
	resp, err := dc.httpClient.Do(req)
	if err != nil {
		return nil, "", fmt.Errorf("download failed: %w", httpsec.RedactErr(err))
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil, "", fmt.Errorf("download failed with status %d", resp.StatusCode)
	}
	maxSize := dc.maxFileSize.Load()
	if resp.ContentLength > maxSize {
		return nil, "", fmt.Errorf("file of %d bytes exceeds the %d byte limit", resp.ContentLength, maxSize)
	}
	data, err := io.ReadAll(io.LimitReader(resp.Body, maxSize+1))
	if err != nil {
		return nil, "", fmt.Errorf("failed to read download: %w", err)
	}
	if int64(len(data)) > maxSize {
		return nil, "", fmt.Errorf("file exceeds the %d byte limit", maxSize)
	}
	return data, resp.Header.Get("Content-Type"), nil
}
