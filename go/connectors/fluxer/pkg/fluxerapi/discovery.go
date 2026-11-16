// SPDX-License-Identifier: AGPL-3.0-or-later

package fluxerapi

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/netip"
	"net/url"
	"strings"
)

// DefaultInstance is the official Fluxer instance.
const DefaultInstance = "https://fluxer.app"

// Policy decides which instance and endpoint URLs are accepted.
type Policy struct {
	// AllowInsecure permits plain http and ws, and local, private or loopback hosts. It exists for
	// local development against a self-hosted instance and must never be set for normal accounts.
	AllowInsecure bool
}

// Endpoints are the public service URLs of an instance.
type Endpoints struct {
	// API is the REST base (endpoints.api_public). Paths are appended after "/v1".
	API string
	// Gateway is the WebSocket URL (endpoints.gateway).
	Gateway string
	// Media is the Media Proxy base (endpoints.media).
	Media string
}

type discoveryDoc struct {
	Endpoints struct {
		APIPublic string `json:"api_public"`
		Gateway   string `json:"gateway"`
		Media     string `json:"media"`
	} `json:"endpoints"`
}

// Discover fetches /.well-known/fluxer from an instance origin and returns its endpoints.
// Clients must use these URLs instead of building them from the origin
// (docs: http-api/instance.mdx "Get instance discovery").
func Discover(ctx context.Context, hc *http.Client, origin string, p Policy) (*Endpoints, error) {
	base, err := NormalizeOrigin(origin, p)
	if err != nil {
		return nil, err
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, base+"/.well-known/fluxer", nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("User-Agent", UserAgent)
	resp, err := hc.Do(req)
	if err != nil {
		return nil, fmt.Errorf("fluxer: instance discovery: %w", RedactErr(err))
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("fluxer: instance discovery returned HTTP %d", resp.StatusCode)
	}
	raw, err := readLimited(resp.Body, MaxJSONBytes)
	if err != nil {
		return nil, fmt.Errorf("fluxer: instance discovery: %w", err)
	}
	var doc discoveryDoc
	if err := json.Unmarshal(raw, &doc); err != nil {
		return nil, fmt.Errorf("fluxer: instance discovery: %w", err)
	}
	ep := &Endpoints{
		API:     strings.TrimRight(doc.Endpoints.APIPublic, "/"),
		Gateway: doc.Endpoints.Gateway,
		Media:   strings.TrimRight(doc.Endpoints.Media, "/"),
	}
	if err := ep.validate(p); err != nil {
		return nil, err
	}
	return ep, nil
}

func (e *Endpoints) validate(p Policy) error {
	if err := p.CheckURL(e.API, "https", "http"); err != nil {
		return fmt.Errorf("fluxer: endpoints.api_public: %w", err)
	}
	if err := p.CheckURL(e.Gateway, "wss", "ws"); err != nil {
		return fmt.Errorf("fluxer: endpoints.gateway: %w", err)
	}
	if e.Media != "" {
		if err := p.CheckURL(e.Media, "https", "http"); err != nil {
			return fmt.Errorf("fluxer: endpoints.media: %w", err)
		}
	}
	return nil
}

// CheckURL accepts an absolute URL whose scheme is secure, or insecure when the policy allows it,
// and whose host is public unless the policy allows local hosts.
func (p Policy) CheckURL(raw, secure, insecure string) error {
	u, err := url.Parse(raw)
	if err != nil || u.Host == "" {
		return errors.New("not an absolute URL")
	}
	switch {
	case u.Scheme == secure:
	case u.Scheme == insecure && p.AllowInsecure:
	default:
		return fmt.Errorf("unexpected scheme %q", u.Scheme)
	}
	return p.checkHost(u.Hostname())
}

// checkHost refuses local names and IP literals in private, loopback or link-local ranges unless
// the policy allows insecure endpoints.
func (p Policy) checkHost(host string) error {
	if p.AllowInsecure {
		return nil
	}
	h := strings.ToLower(strings.TrimSuffix(host, "."))
	if h == "localhost" || strings.HasSuffix(h, ".localhost") {
		return errors.New("local host names are refused")
	}
	addr, err := netip.ParseAddr(h)
	if err != nil {
		return nil
	}
	addr = addr.Unmap()
	if addr.IsLoopback() || addr.IsPrivate() || addr.IsLinkLocalUnicast() ||
		addr.IsLinkLocalMulticast() || addr.IsUnspecified() || addr.IsMulticast() {
		return errors.New("private and loopback addresses are refused")
	}
	return nil
}

// NormalizeOrigin accepts "example.com", "https://example.com/" and similar and returns
// "https://example.com". Plain HTTP and local hosts are only accepted under an insecure policy.
func NormalizeOrigin(origin string, p Policy) (string, error) {
	raw := strings.TrimSpace(origin)
	if raw == "" {
		raw = DefaultInstance
	}
	if !strings.Contains(raw, "://") {
		raw = "https://" + raw
	}
	u, err := url.Parse(raw)
	if err != nil || u.Host == "" {
		return "", errors.New("fluxer: invalid instance URL")
	}
	if err := p.CheckURL(raw, "https", "http"); err != nil {
		return "", fmt.Errorf("fluxer: instance URL: %w", err)
	}
	return u.Scheme + "://" + u.Host, nil
}
