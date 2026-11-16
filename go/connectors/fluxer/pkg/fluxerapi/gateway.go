// SPDX-License-Identifier: AGPL-3.0-or-later

package fluxerapi

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/url"
	"sync"
	"time"

	"github.com/coder/websocket"
)

// Gateway opcodes used by the client (docs: gateway/opcodes-and-close-codes.md "Opcodes").
const (
	opDispatch       = 0
	opHeartbeat      = 1
	opIdentify       = 2
	opResume         = 6
	opReconnect      = 7
	opInvalidSession = 9
	opHello          = 10
	opHeartbeatAck   = 11
	opLazyRequest    = 14
	gatewayVersion   = "1"
	resumeWindow     = 60 * time.Second
)

// Close codes that end the session for good (docs: gateway/opcodes-and-close-codes.md).
const (
	closeUnknownError      = 4000
	closeInvalidToken      = 4004
	closeRateLimited       = 4008
	closeInvalidShard      = 4010
	closeShardingRequired  = 4011
	closeInvalidAPIVersion = 4012
)

// gatewayHTTP performs the WebSocket upgrade for every Gateway connection.
var gatewayHTTP = NewGatewayHTTPClient()

// ErrGatewayAuth means the Gateway rejected the credential. Retrying cannot help.
var ErrGatewayAuth = errors.New("fluxer: gateway rejected the token")

// ErrGatewayPermanent means the Gateway refused the connection for a reason that retrying cannot fix.
var ErrGatewayPermanent = errors.New("fluxer: gateway refused the connection")

// Event is one Dispatch from the Gateway.
type Event struct {
	Type string
	Seq  int64
	Data json.RawMessage
}

// GatewayHandler receives Dispatches in order. It runs on the read goroutine, so it must not block for long.
type GatewayHandler func(ctx context.Context, ev Event)

// StateFunc is told about transient connection problems and recoveries.
type StateFunc func(state GatewayState, err error)

// GatewayState describes the connection state reported to a StateFunc.
type GatewayState int

const (
	// GatewayConnected means READY or RESUMED was received.
	GatewayConnected GatewayState = iota
	// GatewayDisconnected means the socket dropped and a reconnect is scheduled.
	GatewayDisconnected
)

// Gateway keeps one Fluxer Gateway session alive: it identifies, heartbeats, resumes after
// network drops, and re-identifies when the server invalidates the session.
type Gateway struct {
	url     string
	cred    Credential
	handler GatewayHandler
	state   StateFunc

	mu        sync.Mutex
	conn      *websocket.Conn
	sessionID string
	seq       int64
	lastDrop  time.Time
}

// NewGateway prepares a Gateway session for url (endpoints.gateway or GET /gateway/bot).
func NewGateway(url string, cred Credential, handler GatewayHandler, state StateFunc) *Gateway {
	return &Gateway{url: url, cred: cred, handler: handler, state: state}
}

// Run blocks until ctx is cancelled or the server refuses the credential (ErrGatewayAuth).
func (g *Gateway) Run(ctx context.Context) error {
	backoff := time.Second
	for {
		err := g.runConn(ctx)
		if ctx.Err() != nil {
			return ctx.Err()
		}
		if errors.Is(err, ErrGatewayAuth) || errors.Is(err, ErrGatewayPermanent) {
			return err
		}
		g.reportState(GatewayDisconnected, err)
		if err := sleepCtx(ctx, backoff); err != nil {
			return err
		}
		backoff = min(backoff*2, time.Minute)
	}
}

// Send writes a client command (for example Lazy Request) on the live connection.
func (g *Gateway) Send(ctx context.Context, op int, d any) error {
	g.mu.Lock()
	conn := g.conn
	g.mu.Unlock()
	if conn == nil {
		return errors.New("fluxer: gateway is not connected")
	}
	return writeFrame(ctx, conn, op, d)
}

// SubscribeGuilds makes each guild active for the session (Lazy Request, opcode 14), so that a
// user session receives all its messages in large guilds (docs: gateway/event-filtering.md).
func (g *Gateway) SubscribeGuilds(ctx context.Context, guildIDs []string) error {
	subs := make(map[string]map[string]bool, len(guildIDs))
	for _, id := range guildIDs {
		subs[id] = map[string]bool{"active": true, "typing": true}
	}
	return g.Send(ctx, opLazyRequest, map[string]any{"subscriptions": subs})
}

func (g *Gateway) reportState(s GatewayState, err error) {
	if g.state != nil {
		g.state(s, err)
	}
}

// runConn runs one socket from dial to close.
func (g *Gateway) runConn(ctx context.Context) error {
	dialURL, err := withGatewayParams(g.url)
	if err != nil {
		return err
	}
	// The context only covers the dial and upgrade; the connection outlives it.
	dialCtx, cancelDial := context.WithTimeout(ctx, gatewayHandshakeTimeout)
	conn, _, err := websocket.Dial(dialCtx, dialURL, &websocket.DialOptions{
		HTTPClient: gatewayHTTP,
		HTTPHeader: http.Header{"User-Agent": []string{UserAgent}},
	})
	cancelDial()
	if err != nil {
		return fmt.Errorf("fluxer: gateway dial: %w", RedactErr(err))
	}
	conn.SetReadLimit(MaxGatewayReadBytes)
	defer conn.CloseNow()

	interval, err := readHello(ctx, conn)
	if err != nil {
		return err
	}
	g.mu.Lock()
	g.conn = conn
	g.mu.Unlock()
	defer func() {
		g.mu.Lock()
		g.conn = nil
		g.lastDrop = time.Now()
		g.mu.Unlock()
	}()

	if err := g.authenticate(ctx, conn); err != nil {
		return err
	}
	hbCtx, stopHB := context.WithCancel(ctx)
	defer stopHB()
	go g.heartbeat(hbCtx, conn, interval)
	return g.readLoop(ctx, conn)
}

// withGatewayParams adds the connection parameters to the discovered URL, keeping its own query
// (docs: gateway/overview.md "Connection parameters"). Compression is left off.
func withGatewayParams(raw string) (string, error) {
	u, err := url.Parse(raw)
	if err != nil {
		return "", fmt.Errorf("fluxer: gateway URL: %w", err)
	}
	q := u.Query()
	q.Set("v", gatewayVersion)
	q.Set("encoding", "json")
	u.RawQuery = q.Encode()
	return u.String(), nil
}

func readHello(ctx context.Context, conn *websocket.Conn) (time.Duration, error) {
	p, err := readPayload(ctx, conn)
	if err != nil {
		return 0, err
	}
	if p.Op != opHello {
		return 0, fmt.Errorf("fluxer: expected Hello, got opcode %d", p.Op)
	}
	var hello struct {
		HeartbeatInterval int64 `json:"heartbeat_interval"`
	}
	if err := json.Unmarshal(p.D, &hello); err != nil || hello.HeartbeatInterval <= 0 {
		return 0, errors.New("fluxer: malformed Hello")
	}
	return time.Duration(hello.HeartbeatInterval) * time.Millisecond, nil
}

// authenticate sends Resume when the session is still resumable, otherwise Identify.
func (g *Gateway) authenticate(ctx context.Context, conn *websocket.Conn) error {
	g.mu.Lock()
	sid, seq, lastDrop := g.sessionID, g.seq, g.lastDrop
	g.mu.Unlock()
	if sid != "" && !lastDrop.IsZero() && time.Since(lastDrop) < resumeWindow {
		return writeFrame(ctx, conn, opResume, map[string]any{
			"token":      g.cred.Token,
			"session_id": sid,
			"seq":        seq,
		})
	}
	g.mu.Lock()
	g.sessionID = ""
	g.seq = 0
	g.mu.Unlock()
	return writeFrame(ctx, conn, opIdentify, map[string]any{
		"token": g.cred.Token,
		"properties": map[string]any{
			"os":      "Android",
			"browser": "Chirper",
			"device":  "Chirper",
			"mobile":  true,
		},
	})
}

func (g *Gateway) heartbeat(ctx context.Context, conn *websocket.Conn, interval time.Duration) {
	t := time.NewTicker(interval)
	defer t.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-t.C:
			if err := writeFrame(ctx, conn, opHeartbeat, g.heartbeatSeq()); err != nil {
				return
			}
		}
	}
}

// readLoop handles frames until the socket closes or the server asks for a reconnect.
func (g *Gateway) readLoop(ctx context.Context, conn *websocket.Conn) error {
	for {
		p, err := readPayload(ctx, conn)
		if err != nil {
			return g.closeError(err)
		}
		switch p.Op {
		case opDispatch:
			if err := g.dispatch(ctx, p); err != nil {
				return err
			}
		case opHeartbeat:
			if err := writeFrame(ctx, conn, opHeartbeat, g.heartbeatSeq()); err != nil {
				return err
			}
		case opReconnect:
			return errors.New("fluxer: gateway requested reconnect")
		case opInvalidSession:
			g.mu.Lock()
			g.sessionID = ""
			g.mu.Unlock()
			return errors.New("fluxer: gateway invalidated the session")
		case opHeartbeatAck, opHello:
			// Nothing to do.
		}
	}
}

// heartbeatSeq is the value for a heartbeat: the last processed sequence, or null before the first Dispatch
// (docs: gateway/overview.md "Heartbeats").
func (g *Gateway) heartbeatSeq() any {
	g.mu.Lock()
	defer g.mu.Unlock()
	if g.seq == 0 {
		return nil
	}
	return g.seq
}

func (g *Gateway) dispatch(ctx context.Context, p payload) error {
	if p.S == nil {
		return nil
	}
	ev := Event{Type: p.T, Seq: *p.S, Data: p.D}
	switch p.T {
	case "READY":
		var ready Ready
		if err := json.Unmarshal(p.D, &ready); err != nil {
			return fmt.Errorf("fluxer: malformed READY: %w", err)
		}
		g.mu.Lock()
		g.sessionID = ready.SessionID
		g.mu.Unlock()
		g.reportState(GatewayConnected, nil)
	case "RESUMED":
		g.reportState(GatewayConnected, nil)
	}
	g.handler(ctx, ev)
	g.mu.Lock()
	g.seq = ev.Seq
	g.mu.Unlock()
	return nil
}

// closeError maps a Gateway close frame to a sentinel when the close is permanent.
func (g *Gateway) closeError(err error) error {
	switch websocket.CloseStatus(err) {
	case closeInvalidToken:
		return fmt.Errorf("%w: %v", ErrGatewayAuth, err)
	case closeInvalidShard, closeShardingRequired, closeInvalidAPIVersion:
		return fmt.Errorf("%w: %v", ErrGatewayPermanent, err)
	case closeRateLimited, closeUnknownError:
		return fmt.Errorf("fluxer: gateway backing off: %w", err)
	}
	return fmt.Errorf("fluxer: gateway read: %w", err)
}

type payload struct {
	Op int             `json:"op"`
	D  json.RawMessage `json:"d,omitempty"`
	S  *int64          `json:"s,omitempty"`
	T  string          `json:"t,omitempty"`
}

func readPayload(ctx context.Context, conn *websocket.Conn) (payload, error) {
	_, data, err := conn.Read(ctx)
	if err != nil {
		return payload{}, err
	}
	var p payload
	if err := json.Unmarshal(data, &p); err != nil {
		return payload{}, fmt.Errorf("fluxer: malformed gateway payload: %w", err)
	}
	return p, nil
}

func writeFrame(ctx context.Context, conn *websocket.Conn, op int, d any) error {
	frame := struct {
		Op int `json:"op"`
		D  any `json:"d"`
	}{Op: op, D: d}
	raw, err := json.Marshal(frame)
	if err != nil {
		return err
	}
	return conn.Write(ctx, websocket.MessageText, raw)
}
