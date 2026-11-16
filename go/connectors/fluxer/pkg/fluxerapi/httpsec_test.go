// SPDX-License-Identifier: AGPL-3.0-or-later

package fluxerapi

import (
	"bytes"
	"errors"
	"net/http"
	"net/url"
	"strings"
	"testing"
)

func TestCheckRedirect(t *testing.T) {
	https, _ := http.NewRequest(http.MethodGet, "https://example.test/a", nil)
	plain, _ := http.NewRequest(http.MethodGet, "http://example.test/a", nil)
	if err := checkRedirect(https, nil); err != nil {
		t.Errorf("https redirect refused: %v", err)
	}
	if err := checkRedirect(plain, nil); err == nil {
		t.Error("http redirect must be refused")
	}
	via := make([]*http.Request, maxRedirects)
	if err := checkRedirect(https, via); err == nil {
		t.Error("redirect chain of five must be refused")
	}
}

func TestRedactErrDropsURL(t *testing.T) {
	err := &url.Error{Op: "Get", URL: "https://cdn.example.test/a.png?sig=SECRET", Err: errors.New("timeout")}
	got := RedactErr(err).Error()
	if strings.Contains(got, "SECRET") || strings.Contains(got, "cdn.example.test") {
		t.Fatalf("URL leaked: %q", got)
	}
	if !errors.Is(RedactErr(err), err.Err) {
		t.Fatal("cause must stay reachable")
	}
}

func TestReadLimited(t *testing.T) {
	if _, err := readLimited(bytes.NewReader(make([]byte, 8)), 8); err != nil {
		t.Fatalf("body at the limit refused: %v", err)
	}
	if _, err := readLimited(bytes.NewReader(make([]byte, 9)), 8); !errors.Is(err, ErrBodyTooLarge) {
		t.Fatalf("oversized body: err = %v", err)
	}
}
