// Chirper - Discord bridgev2 network connector.
// Copyright (C) 2026 Chirper contributors
//
// Adapted from the remoteauth package of mautrix-discord,
// Copyright (C) 2023 Tulir Asokan, licensed under the GNU Affero General Public License v3 or later.
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Package remoteauth implements Discord's QR-code login ("remote auth") handshake.
//
// The phone-side flow is: the user scans the QR code with the Discord mobile app,
// approves the login there, and this client receives an RSA-encrypted user token.
package remoteauth

import (
	"context"
	"crypto/rand"
	"crypto/rsa"
	"crypto/sha256"
	"crypto/tls"
	"crypto/x509"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"net/http"
	"sync"
	"sync/atomic"
	"time"

	"github.com/bwmarrin/discordgo"
	"github.com/gorilla/websocket"

	"chirper.dev/connectors/discord/pkg/httpsec"
)

const gatewayURL = "wss://remote-auth-gateway.discord.gg/?v=2"

// dialer performs the remote auth WebSocket upgrade with a handshake timeout and a TLS 1.2 floor.
var dialer = &websocket.Dialer{
	Proxy:            http.ProxyFromEnvironment,
	NetDialContext:   (&net.Dialer{Timeout: 10 * time.Second}).DialContext,
	HandshakeTimeout: httpsec.GatewayHandshakeTimeout,
	TLSClientConfig:  &tls.Config{MinVersion: tls.VersionTLS12},
}

// restClient serves the ticket exchange that finishes the handshake.
var restClient = httpsec.NewClient(30*time.Second, httpsec.MaxJSONBytes)

// QRURL is called with the URL that must be rendered as a QR code.
type QRURL func(url string) error

// Login runs the remote auth handshake and returns the user token on success.
// onQR is called once the server hands out a fingerprint. Cancelling ctx aborts the flow.
func Login(ctx context.Context, onQR QRURL) (string, error) {
	privateKey, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		return "", fmt.Errorf("failed to generate key: %w", err)
	}
	header := http.Header{}
	for key, value := range discordgo.DroidWSHeaders {
		header.Set(key, value)
	}
	conn, _, err := dialer.DialContext(ctx, gatewayURL, header)
	if err != nil {
		return "", fmt.Errorf("failed to connect to remote auth gateway: %w", err)
	}
	conn.SetReadLimit(httpsec.MaxGatewayReadBytes)
	c := &session{conn: conn, key: privateKey, onQR: onQR}
	stop := make(chan struct{})
	defer close(stop)
	defer c.close()
	go func() {
		select {
		case <-ctx.Done():
			c.close()
		case <-stop:
		}
	}()
	return c.run(ctx)
}

type session struct {
	conn *websocket.Conn
	key  *rsa.PrivateKey
	onQR QRURL

	writeLock  sync.Mutex
	closeOnce  sync.Once
	heartbeats atomic.Int32
}

func (s *session) close() {
	s.closeOnce.Do(func() {
		s.writeLock.Lock()
		_ = s.conn.WriteMessage(websocket.CloseMessage, websocket.FormatCloseMessage(websocket.CloseNormalClosure, ""))
		s.writeLock.Unlock()
		_ = s.conn.Close()
	})
}

func (s *session) write(v any) error {
	payload, err := json.Marshal(v)
	if err != nil {
		return err
	}
	s.writeLock.Lock()
	defer s.writeLock.Unlock()
	return s.conn.WriteMessage(websocket.TextMessage, payload)
}

func (s *session) decrypt(payload string) ([]byte, error) {
	raw, err := base64.StdEncoding.DecodeString(payload)
	if err != nil {
		return nil, err
	}
	return rsa.DecryptOAEP(sha256.New(), nil, s.key, raw, nil)
}

type packet struct {
	OP string `json:"op"`
}

func (s *session) run(ctx context.Context) (string, error) {
	for {
		_, raw, err := s.conn.ReadMessage()
		if err != nil {
			if ctx.Err() != nil {
				return "", ctx.Err()
			}
			return "", fmt.Errorf("remote auth connection closed: %w", err)
		}
		var head packet
		if err = json.Unmarshal(raw, &head); err != nil {
			return "", err
		}
		token, done, err := s.handle(head.OP, raw)
		if err != nil {
			return "", err
		}
		if done {
			return token, nil
		}
	}
}

// handle processes one server packet. It returns done=true with the token once login completes.
func (s *session) handle(op string, raw []byte) (token string, done bool, err error) {
	switch op {
	case "hello":
		var hello struct {
			HeartbeatInterval int `json:"heartbeat_interval"`
		}
		if err = json.Unmarshal(raw, &hello); err != nil {
			return "", false, err
		}
		go s.heartbeat(time.Duration(hello.HeartbeatInterval) * time.Millisecond)
		return "", false, s.sendInit()
	case "nonce_proof":
		var p struct {
			EncryptedNonce string `json:"encrypted_nonce"`
		}
		if err = json.Unmarshal(raw, &p); err != nil {
			return "", false, err
		}
		nonce, err := s.decrypt(p.EncryptedNonce)
		if err != nil {
			return "", false, err
		}
		proof := sha256.Sum256(nonce)
		return "", false, s.write(map[string]string{
			"op":    "nonce_proof",
			"proof": base64.RawURLEncoding.EncodeToString(proof[:]),
		})
	case "pending_remote_init":
		var p struct {
			Fingerprint string `json:"fingerprint"`
		}
		if err = json.Unmarshal(raw, &p); err != nil {
			return "", false, err
		}
		return "", false, s.onQR("https://discordapp.com/ra/" + p.Fingerprint)
	case "pending_ticket":
		// The user payload (ID, discriminator, avatar, name) is not needed; the token carries the identity.
		return "", false, nil
	case "pending_login":
		var p struct {
			Ticket string `json:"ticket"`
		}
		if err = json.Unmarshal(raw, &p); err != nil {
			return "", false, err
		}
		return s.finish(p.Ticket)
	case "heartbeat_ack":
		s.heartbeats.Add(-1)
		return "", false, nil
	case "cancel":
		return "", false, errors.New("login cancelled on the phone")
	default:
		return "", false, fmt.Errorf("unexpected remote auth op %q", op)
	}
}

func (s *session) sendInit() error {
	raw, err := x509.MarshalPKIXPublicKey(s.key.Public())
	if err != nil {
		return err
	}
	return s.write(map[string]string{
		"op":                 "init",
		"encoded_public_key": base64.RawStdEncoding.EncodeToString(raw),
	})
}

func (s *session) finish(ticket string) (string, bool, error) {
	sess, err := discordgo.New("")
	if err != nil {
		return "", false, err
	}
	sess.LogLevel = discordgo.LogError
	sess.Client = restClient
	encrypted, err := sess.RemoteAuthLogin(ticket)
	if err != nil {
		return "", false, err
	}
	plaintext, err := s.decrypt(encrypted)
	if err != nil {
		return "", false, err
	}
	return string(plaintext), true, nil
}

func (s *session) heartbeat(interval time.Duration) {
	if interval <= 0 {
		return
	}
	ticker := time.NewTicker(interval)
	defer ticker.Stop()
	for range ticker.C {
		if s.heartbeats.Add(1) > 2 {
			s.close()
			return
		}
		if err := s.write(map[string]string{"op": "heartbeat"}); err != nil {
			return
		}
	}
}
