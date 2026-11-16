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
	"errors"
	"fmt"
	"sync"

	"maunium.net/go/mautrix/bridgev2"

	"chirper.dev/connectors/discord/pkg/remoteauth"
)

// qrLogin runs Discord's remote-auth handshake: the QR code is shown first, and the
// token is delivered once the user approves the login on their phone.
type qrLogin struct {
	connector *DiscordConnector
	user      *bridgev2.User

	cancel context.CancelFunc
	qrURL  chan string

	done  chan struct{}
	token string
	err   error
	once  sync.Once
}

var _ bridgev2.LoginProcessDisplayAndWait = (*qrLogin)(nil)

func newQRLogin(dc *DiscordConnector, user *bridgev2.User) *qrLogin {
	return &qrLogin{
		connector: dc,
		user:      user,
		qrURL:     make(chan string, 1),
		done:      make(chan struct{}),
	}
}

func (q *qrLogin) Start(ctx context.Context) (*bridgev2.LoginStep, error) {
	// The handshake outlives the Start call, so it gets its own context that Cancel stops.
	var runCtx context.Context
	runCtx, q.cancel = context.WithCancel(context.Background())
	go func() {
		token, err := remoteauth.Login(runCtx, q.publishQR)
		q.token, q.err = token, err
		close(q.done)
	}()
	select {
	case url := <-q.qrURL:
		return &bridgev2.LoginStep{
			Type:   bridgev2.LoginStepTypeDisplayAndWait,
			StepID: LoginStepIDQR,
			DisplayAndWaitParams: &bridgev2.LoginDisplayAndWaitParams{
				Type:      bridgev2.LoginDisplayTypeQR,
				Data:      url,
				CanCancel: true,
			},
		}, nil
	case <-q.done:
		if q.err == nil {
			return nil, errors.New("Discord ended the QR login before showing a code")
		}
		return nil, fmt.Errorf("failed to start QR login: %w", q.err)
	case <-ctx.Done():
		q.Cancel()
		return nil, context.Cause(ctx)
	}
}

func (q *qrLogin) publishQR(url string) error {
	select {
	case q.qrURL <- url:
	default:
	}
	return nil
}

func (q *qrLogin) Wait(ctx context.Context) (*bridgev2.LoginStep, error) {
	select {
	case <-q.done:
	case <-ctx.Done():
		return nil, context.Cause(ctx)
	}
	if q.err != nil {
		return nil, fmt.Errorf("QR login failed: %w", q.err)
	}
	return q.connector.finishLogin(ctx, q.user, q.token, false)
}

func (q *qrLogin) Cancel() {
	q.once.Do(func() {
		if q.cancel != nil {
			q.cancel()
		}
	})
}
