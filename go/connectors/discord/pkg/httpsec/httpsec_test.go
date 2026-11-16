// Chirper - Discord bridgev2 network connector.
// Copyright (C) 2026 Chirper contributors
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

package httpsec

import (
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
)

func TestClientCapsBodies(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(strings.Repeat("x", 9)))
	}))
	defer srv.Close()
	resp, err := NewClient(0, 8).Get(srv.URL)
	if err != nil {
		if !errors.Is(err, ErrBodyTooLarge) {
			t.Fatalf("Get err = %v, want ErrBodyTooLarge", err)
		}
		return
	}
	defer resp.Body.Close()
	if _, err = io.ReadAll(resp.Body); !errors.Is(err, ErrBodyTooLarge) {
		t.Fatalf("read err = %v, want ErrBodyTooLarge", err)
	}
}

func TestClientAllowsBodyAtLimit(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(strings.Repeat("x", 8)))
	}))
	defer srv.Close()
	resp, err := NewClient(0, 8).Get(srv.URL)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	data, err := io.ReadAll(resp.Body)
	if err != nil || len(data) != 8 {
		t.Fatalf("got %d bytes, err %v", len(data), err)
	}
}

func TestRequireHTTPS(t *testing.T) {
	if err := RequireHTTPS("https://cdn.discordapp.com/a.png"); err != nil {
		t.Errorf("https refused: %v", err)
	}
	for _, raw := range []string{"http://cdn.discordapp.com/a.png", "cdn.discordapp.com/a.png", ""} {
		if RequireHTTPS(raw) == nil {
			t.Errorf("RequireHTTPS(%q) must fail", raw)
		}
	}
}

func TestCheckRedirect(t *testing.T) {
	https, _ := http.NewRequest(http.MethodGet, "https://example.test/", nil)
	plain, _ := http.NewRequest(http.MethodGet, "http://example.test/", nil)
	if checkRedirect(https, nil) != nil {
		t.Error("https redirect refused")
	}
	if checkRedirect(plain, nil) == nil {
		t.Error("http redirect must be refused")
	}
	if checkRedirect(https, make([]*http.Request, maxRedirects)) == nil {
		t.Error("sixth hop must be refused")
	}
}

func TestRedactErrDropsSignedURL(t *testing.T) {
	cause := errors.New("connection reset")
	err := RedactErr(&url.Error{Op: "Get", URL: "https://cdn.example.test/a?sig=SECRET", Err: cause})
	if strings.Contains(err.Error(), "SECRET") {
		t.Fatalf("query leaked: %q", err)
	}
	if !errors.Is(err, cause) {
		t.Fatal("cause must stay reachable")
	}
}
