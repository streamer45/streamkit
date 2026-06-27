// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

package main

import (
	"fmt"
	"net/http"
	"strconv"
	"time"

	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"
)

// Buckets span sub-100ms rejections up to a 60s clip render / slow session start.
var latencyBuckets = []float64{0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10, 30, 60}

// sessionLifetimeBuckets span a quick reload up to the max-lifetime cap.
var sessionLifetimeBuckets = []float64{1, 5, 15, 30, 60, 120, 300, 600, 1800}

// codeClientClosed mirrors nginx's 499: the handler returned without writing a
// response, typically because the client disconnected mid-request.
const codeClientClosed = 499

// knownMethods bounds the requests_total method label so a client cannot mint
// unbounded time series via arbitrary RFC method tokens.
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

// codeLabel keeps canonical HTTP statuses (and 499) but folds anything else to
// its class (e.g. "5xx") so an odd backend code cannot explode label cardinality.
func codeLabel(code int) string {
	if code == codeClientClosed || http.StatusText(code) != "" {
		return strconv.Itoa(code)
	}
	if code >= 100 && code < 600 {
		return fmt.Sprintf("%dxx", code/100)
	}
	return "other"
}

var (
	requestsTotal = promauto.NewCounterVec(prometheus.CounterOpts{
		Name: "webcapture_requests_total",
		Help: "Total web-capture requests by endpoint (clip/cast), method, and HTTP status code.",
	}, []string{"endpoint", "method", "code"})

	requestDuration = promauto.NewHistogramVec(prometheus.HistogramOpts{
		Name:    "webcapture_request_duration_seconds",
		Help:    "Total handler latency in seconds by endpoint. For cast this spans the whole viewer connection.",
		Buckets: latencyBuckets,
	}, []string{"endpoint"})

	inflightRequests = promauto.NewGaugeVec(prometheus.GaugeOpts{
		Name: "webcapture_inflight_requests",
		Help: "In-flight requests by endpoint (received, not yet completed).",
	}, []string{"endpoint"})

	upstreamDuration = promauto.NewHistogramVec(prometheus.HistogramOpts{
		Name:    "webcapture_upstream_duration_seconds",
		Help:    "Time to receive response headers from the skit backend by endpoint.",
		Buckets: latencyBuckets,
	}, []string{"endpoint"})

	rejectedTotal = promauto.NewCounterVec(prometheus.CounterOpts{
		Name: "webcapture_rejected_total",
		Help: "Rejected requests by endpoint and reason.",
	}, []string{"endpoint", "reason"})

	activeSessions = promauto.NewGauge(prometheus.GaugeOpts{
		Name: "webcapture_active_sessions",
		Help: "Live cast sessions the gateway currently owns (one Servo pipeline each).",
	})

	activeViewers = promauto.NewGauge(prometheus.GaugeOpts{
		Name: "webcapture_active_viewers",
		Help: "Active cast viewers across all sessions.",
	})

	sessionsReaped = promauto.NewCounterVec(prometheus.CounterOpts{
		Name: "webcapture_sessions_reaped_total",
		Help: "Cast sessions torn down, by reason (idle, max_lifetime, shutdown).",
	}, []string{"reason"})

	sessionLifetime = promauto.NewHistogram(prometheus.HistogramOpts{
		Name:    "webcapture_session_lifetime_seconds",
		Help:    "Lifetime of a cast session from creation to teardown.",
		Buckets: sessionLifetimeBuckets,
	})
)

type rejectReason string

const (
	reasonBadRequest    rejectReason = "bad_request"
	reasonSSRFBlocked   rejectReason = "ssrf_blocked"
	reasonOverCapacity  rejectReason = "over_capacity"
	reasonUpstreamError rejectReason = "upstream_error"
)

func recordRejection(endpoint string, reason rejectReason) {
	rejectedTotal.WithLabelValues(endpoint, string(reason)).Inc()
}

// statusRecorder captures the response status code while preserving
// http.Flusher so proxied streaming responses keep flushing incrementally. A
// zero code means nothing was written (client closed before any response).
type statusRecorder struct {
	http.ResponseWriter
	code int
}

func (s *statusRecorder) WriteHeader(code int) {
	if s.code == 0 {
		s.code = code
	}
	s.ResponseWriter.WriteHeader(code)
}

func (s *statusRecorder) Write(b []byte) (int, error) {
	if s.code == 0 {
		s.code = http.StatusOK
	}
	return s.ResponseWriter.Write(b)
}

func (s *statusRecorder) Flush() {
	if f, ok := s.ResponseWriter.(http.Flusher); ok {
		f.Flush()
	}
}

// withMetrics records count, latency, and the in-flight gauge for one endpoint.
// Recording runs in a defer so a panicking handler is still counted (as 500)
// before the panic propagates.
func withMetrics(endpoint string, w http.ResponseWriter, r *http.Request, fn func(http.ResponseWriter, *http.Request)) {
	inflightRequests.WithLabelValues(endpoint).Inc()
	rec := &statusRecorder{ResponseWriter: w}
	start := time.Now()

	defer func() {
		p := recover()
		code := rec.code
		switch {
		case p != nil:
			code = http.StatusInternalServerError
		case code == 0:
			code = codeClientClosed
		}
		requestDuration.WithLabelValues(endpoint).Observe(time.Since(start).Seconds())
		requestsTotal.WithLabelValues(endpoint, methodLabel(r.Method), codeLabel(code)).Inc()
		inflightRequests.WithLabelValues(endpoint).Dec()
		if p != nil {
			panic(p)
		}
	}()

	fn(rec, r)
}
