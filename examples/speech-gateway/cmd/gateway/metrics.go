// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

package main

import (
	"net/http"
	"strconv"
	"time"

	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"
)

// Buckets span sub-100ms rejections up to multi-second STT/TTS synthesis.
var latencyBuckets = []float64{0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10, 30}

// codeClientClosed mirrors nginx's 499: the handler returned without writing a
// response, typically because the client disconnected mid-request.
const codeClientClosed = 499

// knownMethods bounds the gateway_requests_total method label; Go accepts any
// RFC token as a method, so any other value folds to "other" to keep a client
// from minting unbounded time series.
var knownMethods = map[string]struct{}{
	http.MethodGet:  {},
	http.MethodHead: {},
	http.MethodPost: {},
	http.MethodPut:  {},
}

func methodLabel(method string) string {
	if _, ok := knownMethods[method]; ok {
		return method
	}
	return "other"
}

var (
	requestsTotal = promauto.NewCounterVec(prometheus.CounterOpts{
		Name: "gateway_requests_total",
		Help: "Total speech-gateway requests by endpoint, method, and HTTP status code.",
	}, []string{"endpoint", "method", "code"})

	requestDuration = promauto.NewHistogramVec(prometheus.HistogramOpts{
		Name:    "gateway_request_duration_seconds",
		Help:    "Total handler latency in seconds by endpoint.",
		Buckets: latencyBuckets,
	}, []string{"endpoint"})

	inflightRequests = promauto.NewGaugeVec(prometheus.GaugeOpts{
		Name: "gateway_inflight_requests",
		Help: "In-flight requests by endpoint (received, not yet completed; includes time queued on the concurrency semaphore).",
	}, []string{"endpoint"})

	upstreamDuration = promauto.NewHistogramVec(prometheus.HistogramOpts{
		Name:    "gateway_upstream_duration_seconds",
		Help:    "Time to receive response headers from the skit backend /api/v1/process by endpoint (excludes streaming the body to the client).",
		Buckets: latencyBuckets,
	}, []string{"endpoint"})

	rejectedTotal = promauto.NewCounterVec(prometheus.CounterOpts{
		Name: "gateway_rejected_total",
		Help: "Rejected requests by endpoint and reason.",
	}, []string{"endpoint", "reason"})
)

// rejectReason records a gateway-side rejection. Reasons are emitted explicitly
// at each rejection site rather than inferred from the status code, so statuses
// forwarded verbatim from the backend are not miscounted as gateway rejections.
type rejectReason string

const (
	reasonBadContentType rejectReason = "bad_content_type"
	reasonTooLarge       rejectReason = "too_large"
	reasonUpstreamError  rejectReason = "upstream_error"
)

func recordRejection(endpoint string, reason rejectReason) {
	rejectedTotal.WithLabelValues(endpoint, string(reason)).Inc()
}

// statusRecorder captures the response status code while preserving
// http.Flusher so proxied responses keep streaming incrementally.
type statusRecorder struct {
	http.ResponseWriter
	code        int
	wroteHeader bool
}

func (s *statusRecorder) WriteHeader(code int) {
	if !s.wroteHeader {
		s.code = code
		s.wroteHeader = true
	}
	s.ResponseWriter.WriteHeader(code)
}

func (s *statusRecorder) Write(b []byte) (int, error) {
	if !s.wroteHeader {
		s.code = http.StatusOK
		s.wroteHeader = true
	}
	return s.ResponseWriter.Write(b)
}

func (s *statusRecorder) Flush() {
	if f, ok := s.ResponseWriter.(http.Flusher); ok {
		f.Flush()
	}
}

// instrument records request count, latency, and the in-flight gauge per
// endpoint. Recording runs in a defer so a panicking handler is still counted
// (as 500) before the panic propagates to the server.
func instrument(endpoint string, next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		inflightRequests.WithLabelValues(endpoint).Inc()
		rec := &statusRecorder{ResponseWriter: w, code: http.StatusOK}
		start := time.Now()

		defer func() {
			p := recover()

			code := rec.code
			switch {
			case p != nil:
				code = http.StatusInternalServerError
			case !rec.wroteHeader:
				code = codeClientClosed
			}

			requestDuration.WithLabelValues(endpoint).Observe(time.Since(start).Seconds())
			requestsTotal.WithLabelValues(endpoint, methodLabel(r.Method), strconv.Itoa(code)).Inc()
			inflightRequests.WithLabelValues(endpoint).Dec()

			if p != nil {
				panic(p)
			}
		}()

		next(rec, r)
	}
}
