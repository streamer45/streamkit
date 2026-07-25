// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

package main

import (
	"net/http"
	"testing"
)

func TestCopyHeadersStripsHopByHopAndConnectionListed(t *testing.T) {
	src := http.Header{
		"Content-Type":      {"video/mp4"},
		"Connection":        {"keep-alive, X-Internal-Token"},
		"Keep-Alive":        {"timeout=5"},
		"Transfer-Encoding": {"chunked"},
		"X-Internal-Token":  {"secret"},
		"Cache-Control":     {"no-store"},
	}
	dst := http.Header{}
	copyHeaders(dst, src)

	if got := dst.Get("Content-Type"); got != "video/mp4" {
		t.Errorf("Content-Type = %q, want forwarded", got)
	}
	if got := dst.Get("Cache-Control"); got != "no-store" {
		t.Errorf("Cache-Control = %q, want forwarded", got)
	}
	for _, h := range []string{"Connection", "Keep-Alive", "Transfer-Encoding", "X-Internal-Token"} {
		if v := dst.Get(h); v != "" {
			t.Errorf("%s = %q, want stripped", h, v)
		}
	}
}
