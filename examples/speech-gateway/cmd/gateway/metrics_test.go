// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

package main

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/prometheus/client_golang/prometheus/promhttp"
)

func TestRejectionReason(t *testing.T) {
	cases := map[int]string{
		http.StatusUnsupportedMediaType:  "bad_content_type",
		http.StatusRequestEntityTooLarge: "too_large",
		http.StatusBadGateway:            "upstream_error",
		http.StatusOK:                    "",
		http.StatusBadRequest:            "",
	}
	for code, want := range cases {
		if got := rejectionReason(code); got != want {
			t.Errorf("rejectionReason(%d) = %q, want %q", code, got, want)
		}
	}
}

func TestInstrumentRecordsRejection(t *testing.T) {
	h := instrument("tts", func(w http.ResponseWriter, _ *http.Request) {
		http.Error(w, "nope", http.StatusUnsupportedMediaType)
	})

	rec := httptest.NewRecorder()
	h(rec, httptest.NewRequest(http.MethodPost, "/tts", nil))
	if rec.Code != http.StatusUnsupportedMediaType {
		t.Fatalf("status = %d, want 415", rec.Code)
	}

	body := scrapeMetrics(t)
	for _, want := range []string{
		`gateway_requests_total{code="415",endpoint="tts",method="POST"}`,
		`gateway_rejected_total{endpoint="tts",reason="bad_content_type"}`,
		`gateway_request_duration_seconds_bucket{endpoint="tts"`,
	} {
		if !strings.Contains(body, want) {
			t.Errorf("metrics output missing %q", want)
		}
	}
}

func TestInstrumentUnwrittenResponseCountedAsClientClosed(t *testing.T) {
	// Handler returns without writing, mirroring the context.Canceled path
	// in handleSTT/handleTTS; it must not be counted as a 200.
	h := instrument("stt", func(http.ResponseWriter, *http.Request) {})

	rec := httptest.NewRecorder()
	h(rec, httptest.NewRequest(http.MethodPost, "/stt", nil))

	body := scrapeMetrics(t)
	if !strings.Contains(body, `gateway_requests_total{code="499",endpoint="stt",method="POST"}`) {
		t.Errorf("expected unwritten response to be counted as code=499; got:\n%s", body)
	}
}

func scrapeMetrics(t *testing.T) string {
	t.Helper()
	rec := httptest.NewRecorder()
	promhttp.Handler().ServeHTTP(rec, httptest.NewRequest(http.MethodGet, "/metrics", nil))
	if rec.Code != http.StatusOK {
		t.Fatalf("/metrics status = %d, want 200", rec.Code)
	}
	return rec.Body.String()
}
