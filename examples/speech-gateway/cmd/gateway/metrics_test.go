// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

package main

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promhttp"
	"github.com/prometheus/client_golang/prometheus/testutil"
	dto "github.com/prometheus/client_model/go"
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

// Backend-controlled codes must fold to a class so a misbehaving backend cannot
// mint unbounded code-label series; canonical codes (and 499) are kept verbatim.
func TestCodeLabel(t *testing.T) {
	cases := map[int]string{
		200:              "200",
		502:              "502",
		codeClientClosed: "499",
		599:              "5xx",
		418:              "418",
		700:              "other",
	}
	for code, want := range cases {
		if got := codeLabel(code); got != want {
			t.Errorf("codeLabel(%d) = %q, want %q", code, got, want)
		}
	}
}

func TestInstrumentRecordsRequest(t *testing.T) {
	const ep = "tts"
	countBefore := testutil.ToFloat64(requestsTotal.WithLabelValues(ep, "POST", "415"))
	durBefore := histSampleCount(t, requestDuration.WithLabelValues(ep))

	h := instrument(ep, func(w http.ResponseWriter, _ *http.Request) {
		http.Error(w, "nope", http.StatusUnsupportedMediaType)
	})
	rec := httptest.NewRecorder()
	h(rec, httptest.NewRequest(http.MethodPost, "/tts", nil))
	if rec.Code != http.StatusUnsupportedMediaType {
		t.Fatalf("status = %d, want 415", rec.Code)
	}

	if got := testutil.ToFloat64(requestsTotal.WithLabelValues(ep, "POST", "415")) - countBefore; got != 1 {
		t.Errorf("requests_total{code=415} delta = %v, want 1", got)
	}
	if got := histSampleCount(t, requestDuration.WithLabelValues(ep)) - durBefore; got != 1 {
		t.Errorf("request_duration sample-count delta = %d, want 1", got)
	}
}

// An unwritten response (the client-disconnect path) must not be counted as 200.
func TestInstrumentUnwrittenResponseCountedAsClientClosed(t *testing.T) {
	const ep = "stt"
	before := testutil.ToFloat64(requestsTotal.WithLabelValues(ep, "POST", "499"))

	h := instrument(ep, func(http.ResponseWriter, *http.Request) {})
	h(httptest.NewRecorder(), httptest.NewRequest(http.MethodPost, "/stt", nil))

	if got := testutil.ToFloat64(requestsTotal.WithLabelValues(ep, "POST", "499")) - before; got != 1 {
		t.Errorf("requests_total{code=499} delta = %v, want 1", got)
	}
}

// Attacker-controlled methods must fold to "other" to bound label cardinality.
func TestInstrumentFoldsUnknownMethod(t *testing.T) {
	const ep = "stt"
	before := testutil.ToFloat64(requestsTotal.WithLabelValues(ep, "other", "405"))

	h := instrument(ep, func(w http.ResponseWriter, _ *http.Request) {
		http.Error(w, "no", http.StatusMethodNotAllowed)
	})
	h(httptest.NewRecorder(), httptest.NewRequest("ZZZRANDOM", "/stt", nil))

	if got := testutil.ToFloat64(requestsTotal.WithLabelValues(ep, "other", "405")) - before; got != 1 {
		t.Errorf("requests_total{method=other} delta = %v, want 1", got)
	}
	if body := scrapeMetrics(t); strings.Contains(body, `method="ZZZRANDOM"`) {
		t.Error("raw attacker method leaked into metrics label")
	}
}

func TestInstrumentPanicCountedAs500(t *testing.T) {
	const ep = "tts"
	before := testutil.ToFloat64(requestsTotal.WithLabelValues(ep, "POST", "500"))

	func() {
		defer func() {
			if recover() == nil {
				t.Error("expected panic to propagate past instrument")
			}
		}()
		h := instrument(ep, func(http.ResponseWriter, *http.Request) { panic("boom") })
		h(httptest.NewRecorder(), httptest.NewRequest(http.MethodPost, "/tts", nil))
	}()

	if got := testutil.ToFloat64(requestsTotal.WithLabelValues(ep, "POST", "500")) - before; got != 1 {
		t.Errorf("requests_total{code=500} delta = %v, want 1", got)
	}
}

func TestRecordRejection(t *testing.T) {
	before := testutil.ToFloat64(rejectedTotal.WithLabelValues("stt", "upstream_error"))
	recordRejection("stt", reasonUpstreamError)
	if got := testutil.ToFloat64(rejectedTotal.WithLabelValues("stt", "upstream_error")) - before; got != 1 {
		t.Errorf("rejected_total{reason=upstream_error} delta = %v, want 1", got)
	}
}

func histSampleCount(t *testing.T, o prometheus.Observer) uint64 {
	t.Helper()
	m, ok := o.(prometheus.Metric)
	if !ok {
		t.Fatalf("observer %T is not a prometheus.Metric", o)
	}
	var pb dto.Metric
	if err := m.Write(&pb); err != nil {
		t.Fatalf("write metric: %v", err)
	}
	return pb.GetHistogram().GetSampleCount()
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
