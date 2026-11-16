// SPDX-License-Identifier: AGPL-3.0-or-later

package fluxerapi

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strconv"
	"time"
)

// UserAgent is sent with every request, as required for well-behaved third-party clients.
const UserAgent = "Chirper (https://github.com/chirper, 0.1)"

const (
	maxRateLimitRetries = 3
	maxRetryAfter       = 60 * time.Second
)

// Client is a REST client bound to one instance and one credential.
type Client struct {
	http *http.Client
	api  string
	cred Credential
}

// NewClient returns a client for the REST API at apiBase (endpoints.api_public).
// A nil httpClient uses NewHTTPClient with a 2 minute timeout.
func NewClient(apiBase string, cred Credential, httpClient *http.Client) *Client {
	if httpClient == nil {
		httpClient = NewHTTPClient(2 * time.Minute)
	}
	return &Client{http: httpClient, api: apiBase, cred: cred}
}

// callOpts describes one REST call.
type callOpts struct {
	query       url.Values
	body        []byte
	contentType string
	captcha     string
	anonymous   bool
}

// call performs a REST request under the API's /v1 prefix and decodes the JSON response into out.
// It honours 429 responses by waiting for retry_after, but never retries a global denial of a
// user session token, because the server revokes that token (docs: topics/rate-limits.md).
func (c *Client) call(ctx context.Context, method, path string, opt callOpts, out any) error {
	for attempt := 0; ; attempt++ {
		err := c.once(ctx, method, path, opt, out)
		var apiErr *APIError
		if !errors.As(err, &apiErr) || apiErr.Status != http.StatusTooManyRequests {
			return err
		}
		if attempt >= maxRateLimitRetries || (apiErr.Global && !c.cred.Bot) {
			return err
		}
		if err := sleepCtx(ctx, apiErr.RetryAfter); err != nil {
			return err
		}
	}
}

func (c *Client) once(ctx context.Context, method, path string, opt callOpts, out any) error {
	target := c.api + "/v1" + path
	if len(opt.query) > 0 {
		target += "?" + opt.query.Encode()
	}
	var body io.Reader
	if opt.body != nil {
		body = bytes.NewReader(opt.body)
	}
	req, err := http.NewRequestWithContext(ctx, method, target, body)
	if err != nil {
		return err
	}
	req.Header.Set("User-Agent", UserAgent)
	req.Header.Set("Accept", "application/json")
	if opt.contentType != "" {
		req.Header.Set("Content-Type", opt.contentType)
	}
	if !opt.anonymous && c.cred.Token != "" {
		req.Header.Set("Authorization", c.cred.authorization())
	}
	if opt.captcha != "" {
		req.Header.Set("X-Captcha-Token", opt.captcha)
	}
	resp, err := c.http.Do(req)
	if err != nil {
		return fmt.Errorf("fluxer: %s %s: %w", method, path, RedactErr(err))
	}
	defer resp.Body.Close()
	raw, err := readLimited(resp.Body, MaxJSONBytes)
	if err != nil {
		return fmt.Errorf("fluxer: reading response: %w", err)
	}
	if resp.StatusCode >= 300 {
		return newAPIError(resp, raw)
	}
	if out == nil || resp.StatusCode == http.StatusNoContent || len(raw) == 0 {
		return nil
	}
	if err := json.Unmarshal(raw, out); err != nil {
		return fmt.Errorf("fluxer: decoding %s response: %w", path, err)
	}
	return nil
}

// jsonBody marshals in as a JSON request body. A nil in produces no body.
func jsonBody(in any) (callOpts, error) {
	if in == nil {
		return callOpts{}, nil
	}
	raw, err := json.Marshal(in)
	if err != nil {
		return callOpts{}, err
	}
	return callOpts{body: raw, contentType: "application/json"}, nil
}

func sleepCtx(ctx context.Context, d time.Duration) error {
	if d <= 0 {
		d = time.Second
	}
	if d > maxRetryAfter {
		d = maxRetryAfter
	}
	t := time.NewTimer(d)
	defer t.Stop()
	select {
	case <-ctx.Done():
		return ctx.Err()
	case <-t.C:
		return nil
	}
}

// APIError is a non-2xx response from the Fluxer API. It never contains the request's credential.
type APIError struct {
	Status  int
	Code    string
	Message string
	// RetryAfter is set for 429 responses.
	RetryAfter time.Duration
	// Global is true when the global rate limit bucket denied the request.
	Global bool
	// CaptchaProvider and CaptchaChallenge are set on CAPTCHA_REQUIRED / INVALID_CAPTCHA.
	CaptchaProvider  string
	CaptchaChallenge json.RawMessage
}

func (e *APIError) Error() string {
	if e.Code == "" {
		return fmt.Sprintf("fluxer: HTTP %d", e.Status)
	}
	return fmt.Sprintf("fluxer: HTTP %d %s: %s", e.Status, e.Code, e.Message)
}

// HasCode reports whether the error is an APIError with the given code.
func HasCode(err error, code string) bool {
	var apiErr *APIError
	return errors.As(err, &apiErr) && apiErr.Code == code
}

func newAPIError(resp *http.Response, raw []byte) *APIError {
	apiErr := &APIError{Status: resp.StatusCode}
	var body struct {
		Code             string          `json:"code"`
		Message          string          `json:"message"`
		Global           bool            `json:"global"`
		RetryAfter       float64         `json:"retry_after"`
		CaptchaProvider  string          `json:"captcha_provider"`
		CaptchaChallenge json.RawMessage `json:"altcha_challenge"`
	}
	if json.Unmarshal(raw, &body) == nil {
		apiErr.Code = body.Code
		apiErr.Message = body.Message
		apiErr.Global = body.Global
		apiErr.CaptchaProvider = body.CaptchaProvider
		apiErr.CaptchaChallenge = body.CaptchaChallenge
		apiErr.RetryAfter = time.Duration(body.RetryAfter * float64(time.Second))
	}
	if apiErr.RetryAfter <= 0 {
		if secs, err := strconv.Atoi(resp.Header.Get("Retry-After")); err == nil {
			apiErr.RetryAfter = time.Duration(secs) * time.Second
		}
	}
	return apiErr
}

// isCaptchaRequired reports whether the server wants a solved ALTCHA challenge.
func (e *APIError) isCaptchaRequired() bool {
	return (e.Code == "CAPTCHA_REQUIRED" || e.Code == "INVALID_CAPTCHA") &&
		e.CaptchaProvider == "altcha" && len(e.CaptchaChallenge) > 0
}
