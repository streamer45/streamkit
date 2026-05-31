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

func TestMethodLabel(t *testing.T) {
	cases := map[string]string{
		http.MethodPost: "POST",
		http.MethodGet:  "GET",
		"BREW":          "other",
		"AAAA1":         "other",
	}
	for method, want := range cases {
		if got := methodLabel(method); got != want {
			t.Errorf("methodLabel(%q) = %q, want %q", method, got, want)
		}
	}
}

func TestInstrumentRecordsRequest(t *testing.T) {
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
		`gateway_request_duration_seconds_bucket{endpoint="tts"`,
	} {
		if !strings.Contains(body, want) {
			t.Errorf("metrics output missing %q", want)
		}
	}
}

// An unwritten response (the context.Canceled path) must not be counted as 200.
func TestInstrumentUnwrittenResponseCountedAsClientClosed(t *testing.T) {
	h := instrument("stt", func(http.ResponseWriter, *http.Request) {})

	rec := httptest.NewRecorder()
	h(rec, httptest.NewRequest(http.MethodPost, "/stt", nil))

	body := scrapeMetrics(t)
	if !strings.Contains(body, `gateway_requests_total{code="499",endpoint="stt",method="POST"}`) {
		t.Errorf("expected unwritten response to be counted as code=499; got:\n%s", body)
	}
}

// Attacker-controlled methods must fold to "other" to bound label cardinality.
func TestInstrumentFoldsUnknownMethod(t *testing.T) {
	h := instrument("stt", func(w http.ResponseWriter, _ *http.Request) {
		http.Error(w, "no", http.StatusMethodNotAllowed)
	})

	rec := httptest.NewRecorder()
	h(rec, httptest.NewRequest("ZZZRANDOM", "/stt", nil))

	body := scrapeMetrics(t)
	if !strings.Contains(body, `gateway_requests_total{code="405",endpoint="stt",method="other"}`) {
		t.Errorf("expected unknown method folded to \"other\"; got:\n%s", body)
	}
	if strings.Contains(body, `method="ZZZRANDOM"`) {
		t.Error("raw attacker method leaked into metrics label")
	}
}

func TestInstrumentPanicCountedAs500(t *testing.T) {
	h := instrument("tts", func(http.ResponseWriter, *http.Request) {
		panic("boom")
	})

	func() {
		defer func() {
			if recover() == nil {
				t.Error("expected panic to propagate past instrument")
			}
		}()
		h(httptest.NewRecorder(), httptest.NewRequest(http.MethodPost, "/tts", nil))
	}()

	body := scrapeMetrics(t)
	if !strings.Contains(body, `gateway_requests_total{code="500",endpoint="tts",method="POST"}`) {
		t.Errorf("expected panicking handler counted as code=500; got:\n%s", body)
	}
}

func TestRecordRejection(t *testing.T) {
	recordRejection("stt", reasonUpstreamError)

	body := scrapeMetrics(t)
	if !strings.Contains(body, `gateway_rejected_total{endpoint="stt",reason="upstream_error"}`) {
		t.Errorf("expected recordRejection to emit gateway_rejected_total; got:\n%s", body)
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
