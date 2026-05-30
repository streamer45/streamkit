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
		Help: "In-flight speech-gateway requests by endpoint.",
	}, []string{"endpoint"})

	upstreamDuration = promauto.NewHistogramVec(prometheus.HistogramOpts{
		Name:    "gateway_upstream_duration_seconds",
		Help:    "Time spent calling the skit backend /api/v1/process by endpoint.",
		Buckets: latencyBuckets,
	}, []string{"endpoint"})

	rejectedTotal = promauto.NewCounterVec(prometheus.CounterOpts{
		Name: "gateway_rejected_total",
		Help: "Rejected requests by endpoint and reason.",
	}, []string{"endpoint", "reason"})
)

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

// instrument records request count, latency, and in-flight gauge per endpoint,
// and maps rejection status codes to gateway_rejected_total reasons.
func instrument(endpoint string, next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		inflightRequests.WithLabelValues(endpoint).Inc()
		defer inflightRequests.WithLabelValues(endpoint).Dec()

		rec := &statusRecorder{ResponseWriter: w, code: http.StatusOK}
		start := time.Now()
		next(rec, r)

		code := rec.code
		if !rec.wroteHeader {
			code = codeClientClosed
		}
		requestDuration.WithLabelValues(endpoint).Observe(time.Since(start).Seconds())
		requestsTotal.WithLabelValues(endpoint, r.Method, strconv.Itoa(code)).Inc()
		if reason := rejectionReason(code); reason != "" {
			rejectedTotal.WithLabelValues(endpoint, reason).Inc()
		}
	}
}

func rejectionReason(code int) string {
	switch code {
	case http.StatusUnsupportedMediaType:
		return "bad_content_type"
	case http.StatusRequestEntityTooLarge:
		return "too_large"
	case http.StatusBadGateway:
		return "upstream_error"
	default:
		return ""
	}
}
