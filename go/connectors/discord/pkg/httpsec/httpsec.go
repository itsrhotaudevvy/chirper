// Chirper - Discord bridgev2 network connector.
// Copyright (C) 2026 Chirper contributors
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Package httpsec holds the HTTP and WebSocket limits shared by the Discord connector: timeouts,
// TLS 1.2 as the floor, https-only redirects, bounded response bodies and redacted transport errors.
package httpsec

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
	// GatewayHandshakeTimeout bounds the dial and WebSocket upgrade of one gateway connection.
	GatewayHandshakeTimeout = 20 * time.Second
	maxRedirects            = 5

	// MaxJSONBytes bounds every JSON response body read through a client from NewClient.
	MaxJSONBytes = 8 << 20
	// MaxMediaBytes bounds one downloaded attachment or image.
	MaxMediaBytes = 100 << 20
	// MaxGatewayReadBytes bounds one gateway frame.
	MaxGatewayReadBytes = 16 << 20
)

// ErrBodyTooLarge means a response body is larger than the client allows.
var ErrBodyTooLarge = errors.New("discord: response body exceeds the size limit")

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

// NewClient returns a client with an overall timeout and a hard cap on every response body. Bodies
// over maxBody fail with ErrBodyTooLarge, including bodies read internally by discordgo.
func NewClient(timeout time.Duration, maxBody int64) *http.Client {
	return &http.Client{
		Timeout:       timeout,
		Transport:     &limitTransport{base: newTransport(), max: maxBody},
		CheckRedirect: checkRedirect,
	}
}

// NewGatewayClient returns a client for WebSocket upgrades. It has no overall timeout because the
// connection is long-lived; only the dial, TLS and header timeouts apply.
func NewGatewayClient() *http.Client {
	return &http.Client{Transport: newTransport(), CheckRedirect: checkRedirect}
}

// RequireHTTPS returns an error unless raw is an absolute https URL.
func RequireHTTPS(raw string) error {
	u, err := url.Parse(raw)
	if err != nil || u.Host == "" || u.Scheme != "https" {
		return errors.New("discord: URL must be an absolute https URL")
	}
	return nil
}

func checkRedirect(req *http.Request, via []*http.Request) error {
	if len(via) >= maxRedirects {
		return fmt.Errorf("discord: stopped after %d redirects", maxRedirects)
	}
	if req.URL.Scheme != "https" {
		return errors.New("discord: refusing a redirect to a non-https URL")
	}
	return nil
}

// RedactErr removes the request URL from transport errors. Attachment URLs carry signatures in
// their query, so the URL must not reach logs or the user.
func RedactErr(err error) error {
	var urlErr *url.Error
	if errors.As(err, &urlErr) {
		return fmt.Errorf("%s request: %w", urlErr.Op, urlErr.Err)
	}
	return err
}

// limitTransport wraps response bodies so that reads past max fail instead of growing without bound.
type limitTransport struct {
	base http.RoundTripper
	max  int64
}

func (t *limitTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	resp, err := t.base.RoundTrip(req)
	if err != nil {
		return nil, err
	}
	if resp.ContentLength > t.max {
		_ = resp.Body.Close()
		return nil, ErrBodyTooLarge
	}
	resp.Body = &limitedBody{rc: resp.Body, lr: &io.LimitedReader{R: resp.Body, N: t.max + 1}}
	return resp, nil
}

// CloseIdleConnections forwards to the base transport so that http.Client.CloseIdleConnections works.
func (t *limitTransport) CloseIdleConnections() {
	if c, ok := t.base.(interface{ CloseIdleConnections() }); ok {
		c.CloseIdleConnections()
	}
}

type limitedBody struct {
	rc io.ReadCloser
	lr *io.LimitedReader
}

func (b *limitedBody) Read(p []byte) (int, error) {
	if b.lr.N <= 0 {
		return 0, ErrBodyTooLarge
	}
	return b.lr.Read(p)
}

func (b *limitedBody) Close() error {
	return b.rc.Close()
}
