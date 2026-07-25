// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

package main

import (
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
)

// A browser request (Accept: text/html) must get the autoplay player page for
// both clip and cast, without touching the backend/render path. CLI clients
// (no text/html in Accept) fall through to the raw-video path.
func TestServePlayerPageForBrowser(t *testing.T) {
	u, _ := url.Parse("https://example.com/")
	gw := &gateway{}

	cases := []struct {
		name    string
		serve   func(http.ResponseWriter, *http.Request, *url.URL, captureOpts)
		titleID string
	}{
		{"clip", gw.serveClip, "web-clip"},
		{"cast", gw.serveCast, "web-cast"},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			r := httptest.NewRequest(http.MethodGet, "/"+tc.name+"/example.com", nil)
			r.Header.Set("Accept", "text/html,application/xhtml+xml")
			w := httptest.NewRecorder()

			tc.serve(w, r, u, captureOpts{})

			resp := w.Result()
			if resp.StatusCode != http.StatusOK {
				t.Fatalf("status = %d, want 200", resp.StatusCode)
			}
			if ct := resp.Header.Get("Content-Type"); !strings.HasPrefix(ct, "text/html") {
				t.Fatalf("Content-Type = %q, want text/html", ct)
			}
			body := w.Body.String()
			for _, want := range []string{"<video", "autoplay", tc.titleID, "example.com"} {
				if !strings.Contains(body, want) {
					t.Errorf("body missing %q\nbody: %s", want, body)
				}
			}
		})
	}
}

func TestAcceptsHTML(t *testing.T) {
	cases := []struct {
		accept string
		want   bool
	}{
		{"text/html,application/xhtml+xml", true},
		{"*/*", false},
		{"", false},
		{"application/json", false},
	}
	for _, tc := range cases {
		r := httptest.NewRequest(http.MethodGet, "/clip/example.com", nil)
		if tc.accept != "" {
			r.Header.Set("Accept", tc.accept)
		}
		if got := acceptsHTML(r); got != tc.want {
			t.Errorf("acceptsHTML(%q) = %v, want %v", tc.accept, got, tc.want)
		}
	}
}
