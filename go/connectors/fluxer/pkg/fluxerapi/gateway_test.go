// SPDX-License-Identifier: AGPL-3.0-or-later

package fluxerapi

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/coder/websocket"
)

type fakeFrame struct {
	Op int             `json:"op"`
	D  json.RawMessage `json:"d"`
}

// frameReader pumps client frames into a channel so that heartbeats never block the test.
type frameReader struct {
	t          *testing.T
	frames     chan fakeFrame
	heartbeats *int
	mu         *sync.Mutex
}

func newFrameReader(t *testing.T, ctx context.Context, conn *websocket.Conn, heartbeats *int, mu *sync.Mutex) *frameReader {
	fr := &frameReader{t: t, frames: make(chan fakeFrame, 64), heartbeats: heartbeats, mu: mu}
	go func() {
		defer close(fr.frames)
		for {
			_, data, err := conn.Read(ctx)
			if err != nil {
				return
			}
			var f fakeFrame
			if err := json.Unmarshal(data, &f); err != nil {
				return
			}
			if f.Op == opHeartbeat {
				fr.mu.Lock()
				*fr.heartbeats++
				fr.mu.Unlock()
				continue
			}
			fr.frames <- f
		}
	}()
	return fr
}

// next returns the next non-heartbeat client frame.
func (fr *frameReader) next() fakeFrame {
	fr.t.Helper()
	select {
	case f, ok := <-fr.frames:
		if !ok {
			fr.t.Fatal("client closed before sending the expected frame")
		}
		return f
	case <-time.After(5 * time.Second):
		fr.t.Fatal("timed out waiting for a client frame")
	}
	return fakeFrame{}
}

func writeJSON(t *testing.T, ctx context.Context, conn *websocket.Conn, v any) {
	t.Helper()
	data, err := json.Marshal(v)
	if err != nil {
		t.Fatal(err)
	}
	if err := conn.Write(ctx, websocket.MessageText, data); err != nil {
		t.Fatalf("fake gateway write: %v", err)
	}
}

// TestGatewayIdentifyDispatchResumeAndAuthFailure drives a full lifecycle against a fake Gateway:
// Hello -> Identify -> READY -> MESSAGE_CREATE -> drop -> Resume -> RESUMED -> auth failure.
func TestGatewayIdentifyDispatchResumeAndAuthFailure(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	var mu sync.Mutex
	var connections int
	var heartbeats int
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		conn, err := websocket.Accept(w, r, nil)
		if err != nil {
			t.Errorf("accept: %v", err)
			return
		}
		defer conn.CloseNow()
		mu.Lock()
		connections++
		n := connections
		mu.Unlock()

		fr := newFrameReader(t, ctx, conn, &heartbeats, &mu)
		writeJSON(t, ctx, conn, map[string]any{"op": 10, "d": map[string]any{"heartbeat_interval": 20}})
		switch n {
		case 1:
			id := fr.next()
			if id.Op != opIdentify || !strings.Contains(string(id.D), `"token":"user-tok"`) {
				t.Errorf("first frame = op %d %s, want Identify with token", id.Op, id.D)
			}
			writeJSON(t, ctx, conn, map[string]any{"op": 0, "t": "READY", "s": 1,
				"d": map[string]any{"session_id": "sess-1", "user": map[string]any{"id": "7", "username": "me", "discriminator": "0001"}, "guilds": []any{}, "private_channels": []any{}}})
			writeJSON(t, ctx, conn, map[string]any{"op": 0, "t": "MESSAGE_CREATE", "s": 2, "d": map[string]any{"id": "100"}})
			// Drop the socket with a resumable close code.
			time.Sleep(100 * time.Millisecond) // let a few heartbeats arrive before dropping
			conn.Close(websocket.StatusCode(4000), "drain")
		case 2:
			res := fr.next()
			if res.Op != opResume || !strings.Contains(string(res.D), `"session_id":"sess-1"`) || !strings.Contains(string(res.D), `"seq":2`) {
				t.Errorf("resume frame = op %d %s", res.Op, res.D)
			}
			writeJSON(t, ctx, conn, map[string]any{"op": 0, "t": "RESUMED", "s": 2, "d": map[string]any{}})
			writeJSON(t, ctx, conn, map[string]any{"op": 0, "t": "MESSAGE_CREATE", "s": 3, "d": map[string]any{"id": "101"}})
			conn.Close(websocket.StatusCode(4004), "Invalid token")
			_ = res
		}
	}))
	defer srv.Close()

	wsURL := "ws" + strings.TrimPrefix(srv.URL, "http")
	var got []string
	var seqs []int64
	gw := NewGateway(wsURL, Credential{Token: "user-tok"},
		func(_ context.Context, ev Event) {
			got = append(got, ev.Type)
			seqs = append(seqs, ev.Seq)
		}, nil)

	err := gw.Run(ctx)
	if !errors.Is(err, ErrGatewayAuth) {
		t.Fatalf("Run error = %v, want ErrGatewayAuth", err)
	}
	want := []string{"READY", "MESSAGE_CREATE", "RESUMED", "MESSAGE_CREATE"}
	if strings.Join(got, ",") != strings.Join(want, ",") {
		t.Fatalf("events = %v, want %v", got, want)
	}
	if seqs[len(seqs)-1] != 3 {
		t.Fatalf("last seq = %d, want 3", seqs[len(seqs)-1])
	}
	mu.Lock()
	defer mu.Unlock()
	if connections != 2 {
		t.Fatalf("connections = %d, want 2", connections)
	}
	if heartbeats == 0 {
		t.Fatal("client never heartbeated")
	}
}

func TestWithGatewayParamsKeepsExistingQuery(t *testing.T) {
	got, err := withGatewayParams("wss://gw.example.test/?shard=x")
	if err != nil {
		t.Fatal(err)
	}
	for _, want := range []string{"v=1", "encoding=json", "shard=x"} {
		if !strings.Contains(got, want) {
			t.Fatalf("%q missing %q", got, want)
		}
	}
}
