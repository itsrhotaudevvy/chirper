// SPDX-License-Identifier: AGPL-3.0-or-later

package fluxerapi

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestDiscoverReadsEndpoints(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/.well-known/fluxer" {
			http.NotFound(w, r)
			return
		}
		_, _ = w.Write([]byte(`{"endpoints":{"api_public":"https://api.example.test/","gateway":"wss://gw.example.test","media":"https://media.example.test"}}`))
	}))
	defer srv.Close()

	ep, err := Discover(context.Background(), srv.Client(), srv.URL, Policy{AllowInsecure: true})
	if err != nil {
		t.Fatal(err)
	}
	if ep.API != "https://api.example.test" || ep.Gateway != "wss://gw.example.test" || ep.Media != "https://media.example.test" {
		t.Fatalf("endpoints = %+v", ep)
	}
}

func TestDiscoverRejectsBadGatewayScheme(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{"endpoints":{"api_public":"https://api.example.test","gateway":"https://gw.example.test"}}`))
	}))
	defer srv.Close()
	if _, err := Discover(context.Background(), srv.Client(), srv.URL, Policy{AllowInsecure: true}); err == nil {
		t.Fatal("expected error for https gateway scheme")
	}
}

func TestNormalizeOrigin(t *testing.T) {
	cases := map[string]string{
		"":                      "https://fluxer.app",
		"fluxer.example.org":    "https://fluxer.example.org",
		"https://chat.example/": "https://chat.example",
	}
	for in, want := range cases {
		got, err := NormalizeOrigin(in, Policy{})
		if err != nil || got != want {
			t.Errorf("NormalizeOrigin(%q) = %q, %v; want %q", in, got, err, want)
		}
	}
	refused := []string{
		"http://chat.example",
		"http://localhost:8080/foo",
		"https://127.0.0.1:8443",
		"https://10.1.2.3",
		"https://192.168.0.10",
		"https://[fe80::1]",
		"https://[::1]:8080",
	}
	for _, in := range refused {
		if _, err := NormalizeOrigin(in, Policy{}); err == nil {
			t.Errorf("NormalizeOrigin(%q) must be refused without the dev policy", in)
		}
	}
	if got, err := NormalizeOrigin("http://localhost:8080/foo", Policy{AllowInsecure: true}); err != nil || got != "http://localhost:8080" {
		t.Errorf("dev policy: got %q, %v", got, err)
	}
}

func TestDiscoverRefusesPrivateEndpoints(t *testing.T) {
	srv := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{"endpoints":{"api_public":"https://10.0.0.5/","gateway":"wss://gw.example.test"}}`))
	}))
	defer srv.Close()
	if _, err := Discover(context.Background(), srv.Client(), srv.URL, Policy{}); err == nil {
		t.Fatal("expected a private API host to be refused")
	}
}
