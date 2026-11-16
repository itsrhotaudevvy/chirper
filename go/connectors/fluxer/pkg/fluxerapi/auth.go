// SPDX-License-Identifier: AGPL-3.0-or-later

package fluxerapi

import (
	"context"
	"errors"
	"net/http"
)

// maxCaptchaRounds bounds how many fresh challenges are solved for one request.
const maxCaptchaRounds = 3

// ErrCaptchaFailed is returned when the server keeps rejecting solved challenges.
var ErrCaptchaFailed = errors.New("fluxer: could not pass the CAPTCHA challenge")

// LoginRequest is the body of POST /auth/login (docs: http-api/authentication.mdx).
type LoginRequest struct {
	Login    string `json:"login"`
	Password string `json:"password"`
}

// LoginResult is either a finished sign-in (Token set) or an MFA challenge (MFA set).
type LoginResult struct {
	Token          string   `json:"token"`
	UserID         string   `json:"user_id"`
	User           User     `json:"user"`
	MFA            bool     `json:"mfa"`
	Ticket         string   `json:"ticket"`
	AllowedMethods []string `json:"allowed_methods"`
	TOTP           bool     `json:"totp"`
	WebAuthn       bool     `json:"webauthn"`
	BackupCodes    bool     `json:"backup_codes"`
}

// Login signs in with a login and password (POST /auth/login). Unauthenticated; solves the
// ALTCHA challenge when the instance asks for one.
func (c *Client) Login(ctx context.Context, req LoginRequest) (*LoginResult, error) {
	opt, err := jsonBody(req)
	if err != nil {
		return nil, err
	}
	opt.anonymous = true
	var res LoginResult
	err = c.withCaptcha(ctx, func(captcha string) error {
		o := opt
		o.captcha = captcha
		res = LoginResult{}
		return c.call(ctx, http.MethodPost, "/auth/login", o, &res)
	})
	if err != nil {
		return nil, err
	}
	return &res, nil
}

// LoginTOTP completes an MFA sign-in with an authenticator or backup code
// (POST /auth/login/mfa/totp). The ticket comes from Login and is valid for 5 minutes.
func (c *Client) LoginTOTP(ctx context.Context, ticket, code string) (*LoginResult, error) {
	opt, err := jsonBody(map[string]string{"ticket": ticket, "code": code})
	if err != nil {
		return nil, err
	}
	opt.anonymous = true
	var res LoginResult
	if err := c.call(ctx, http.MethodPost, "/auth/login/mfa/totp", opt, &res); err != nil {
		return nil, err
	}
	return &res, nil
}

// withCaptcha runs call, and when the server answers CAPTCHA_REQUIRED or INVALID_CAPTCHA with
// an ALTCHA challenge, solves it and retries with X-Captcha-Token.
func (c *Client) withCaptcha(ctx context.Context, call func(captcha string) error) error {
	captcha := ""
	for round := 0; round < maxCaptchaRounds; round++ {
		err := call(captcha)
		var apiErr *APIError
		if err == nil || !errors.As(err, &apiErr) || !apiErr.isCaptchaRequired() {
			return err
		}
		solved, serr := SolveAltcha(ctx, apiErr.CaptchaChallenge)
		if serr != nil {
			return serr
		}
		captcha = solved
	}
	return ErrCaptchaFailed
}
