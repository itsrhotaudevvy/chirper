// SPDX-License-Identifier: AGPL-3.0-or-later

package fluxerapi

import (
	"crypto/tls"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"time"
)

const (
	dialTimeout           = 10 * time.Second
	tlsHandshakeTimeout   = 10 * time.Second
	responseHeaderTimeout = 20 * time.Second
	// gatewayHandshakeTimeout bounds the dial and WebSocket upgrade of one Gateway connection.
	gatewayHandshakeTimeout = 20 * time.Second
	maxRedirects            = 5

	// MaxJSONBytes bounds every JSON response body the client reads.
	MaxJSONBytes = 8 << 20
	// MaxGatewayReadBytes bounds one Gateway frame.
	MaxGatewayReadBytes = 16 << 20
)

// ErrBodyTooLarge means a response body is larger than the caller allows.
var ErrBodyTooLarge = errors.New("fluxer: response body exceeds the size limit")

// newTransport returns a transport with dial, TLS and response-header timeouts and TLS 1.2 as the floor.
func newTransport() *http.Transport {
	return &http.Transport{
		Proxy:                 http.ProxyFromEnvironment,
		DialContext:           (&net.Dialer{Timeout: dialTimeout}).DialContext,
		TLSHandshakeTimeout:   tlsHandshakeTimeout,
		ResponseHeaderTimeout: responseHeaderTimeout,
		ForceAttemptHTTP2:     true,
		TLSClientConfig:       &tls.Config{MinVersion: tls.VersionTLS12},
	}
}

// NewHTTPClient returns a client for request/response calls with an overall timeout, which
// bounds the whole exchange including the body. Redirects are limited to https and five hops.
func NewHTTPClient(timeout time.Duration) *http.Client {
	return &http.Client{Timeout: timeout, Transport: newTransport(), CheckRedirect: checkRedirect}
}

// NewGatewayHTTPClient returns a client for WebSocket upgrades. It has no overall timeout, because
// a Gateway connection is long-lived; only the connect and header timeouts apply.
func NewGatewayHTTPClient() *http.Client {
	return &http.Client{Transport: newTransport(), CheckRedirect: checkRedirect}
}

func checkRedirect(req *http.Request, via []*http.Request) error {
	if len(via) >= maxRedirects {
		return fmt.Errorf("fluxer: stopped after %d redirects", maxRedirects)
	}
	if req.URL.Scheme != "https" {
		return errors.New("fluxer: refusing a redirect to a non-https URL")
	}
	return nil
}

// RedactErr removes the request URL from transport errors. Media and signed attachment URLs carry
// signatures in their query, so the URL must not reach logs or the user.
func RedactErr(err error) error {
	var urlErr *url.Error
	if errors.As(err, &urlErr) {
		return fmt.Errorf("%s request: %w", urlErr.Op, urlErr.Err)
	}
	return err
}

// readLimited reads at most limit bytes and fails with ErrBodyTooLarge when the body is longer.
func readLimited(r io.Reader, limit int64) ([]byte, error) {
	data, err := io.ReadAll(io.LimitReader(r, limit+1))
	if err != nil {
		return nil, err
	}
	if int64(len(data)) > limit {
		return nil, ErrBodyTooLarge
	}
	return data, nil
}
