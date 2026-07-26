// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Gateway renders web pages to video via a StreamKit backend: clip.* returns a
// finite MP4 (oneshot), cast.* returns a live WebM stream (dynamic session).
package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"log"
	"math"
	"net/http"
	"net/url"
	"os"
	"os/signal"
	"strconv"
	"syscall"
	"time"

	"github.com/prometheus/client_golang/prometheus/promhttp"
)

const (
	defaultSkitURL    = "http://127.0.0.1:4545"
	defaultListenAddr = ":8080"

	captureFPS = 30

	// Capture resolution: pages render AND encode at this size (1:1, no
	// downscale, so text stays crisp). Override per request with res=WxH or
	// globally with --resolution / GATEWAY_RESOLUTION.
	defaultResolution = "1920x1080"

	// Video bitrate defaults (kbps), tuned for crisp 1080p screen content.
	defaultClipBitrateKbps = 10000
	defaultCastBitrateKbps = 6000

	// Software encoders by default so it runs anywhere (no GPU); hardware is
	// opt-in via --clip-encoder / --cast-encoder.
	defaultClipEncoder = "h264-sw"
	defaultCastEncoder = "vp9-sw"

	// Cap on how long the renderer holds the first frame waiting for the
	// page to become ready (servo load_timeout_secs). The plugin default
	// (30s) assumes offline captures; for an interactive gateway every
	// second here is dead air before the first byte, and ad-heavy pages
	// never fire their load event at all. Must stay below mseReadyTimeout
	// or cast viewers 503 before the stream can produce its init segment.
	defaultLoadTimeoutSecs = 5
)

type config struct {
	listenAddr     string
	skitURL        string
	authToken      string
	maxConcurrency int
	clipDefaultDur time.Duration
	clipMaxDur     time.Duration
	resW           int
	resH           int
	clipBitrate    int
	castBitrate    int
	clipEncoder    string
	castEncoder    string
	loadTimeout    int
	maxSessions    int
	maxViewers     int
	idleTTL        time.Duration
	maxLifetime    time.Duration
}

type gateway struct {
	client          *http.Client // oneshot + skit control calls
	streamClient    *http.Client // long-lived MSE proxy
	skitURL         string
	clipSem         chan struct{}
	clipDefaultDur  time.Duration
	clipMaxDur      time.Duration
	resW            int
	resH            int
	clipBitrate     int
	castBitrate     int
	clipEnc         encoderProfile
	castEnc         encoderProfile
	loadTimeout     int
	mseReadyTimeout time.Duration
	skit            *skitClient
	sessions        *sessionManager
}

func main() {
	cfg := loadConfig()

	clipEnc, err := lookupEncoder(clipEncoders, cfg.clipEncoder)
	if err != nil {
		log.Fatalf("invalid --clip-encoder: %v", err)
	}
	castEnc, err := lookupEncoder(castEncoders, cfg.castEncoder)
	if err != nil {
		log.Fatalf("invalid --cast-encoder: %v", err)
	}

	ctrlClient := newHTTPClient(false, 0)
	skit := &skitClient{client: ctrlClient, baseURL: cfg.skitURL, token: cfg.authToken}
	gw := &gateway{
		client:          ctrlClient,
		streamClient:    newHTTPClient(true, 10*time.Second),
		skitURL:         cfg.skitURL,
		clipSem:         make(chan struct{}, cfg.maxConcurrency),
		clipDefaultDur:  cfg.clipDefaultDur,
		clipMaxDur:      cfg.clipMaxDur,
		resW:            cfg.resW,
		resH:            cfg.resH,
		clipBitrate:     cfg.clipBitrate,
		castBitrate:     cfg.castBitrate,
		clipEnc:         clipEnc,
		castEnc:         castEnc,
		loadTimeout:     cfg.loadTimeout,
		mseReadyTimeout: 8 * time.Second,
		skit:            skit,
		sessions:        newSessionManager(skit, cfg.maxSessions, cfg.maxViewers, cfg.idleTTL, cfg.maxLifetime),
	}
	log.Printf("encoders: clip=%s cast=%s", cfg.clipEncoder, cfg.castEncoder)

	srv := &http.Server{
		Addr:              cfg.listenAddr,
		Handler:           logging(gw.router()),
		ReadHeaderTimeout: 5 * time.Second,
	}

	reaperCtx, cancelReaper := context.WithCancel(context.Background())
	go gw.sessions.runReaper(reaperCtx, 10*time.Second)

	done := make(chan struct{})
	go func() {
		sigCh := make(chan os.Signal, 1)
		signal.Notify(sigCh, os.Interrupt, syscall.SIGTERM)
		<-sigCh
		log.Printf("shutdown signal received; draining and tearing down sessions")
		cancelReaper()

		shutCtx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		if err := srv.Shutdown(shutCtx); err != nil {
			log.Printf("graceful shutdown timed out (%v); forcing", err)
			_ = srv.Close()
		}
		// Tear down owned sessions so a restart never leaks Servo pipelines.
		destroyCtx, cancel2 := context.WithTimeout(context.Background(), 15*time.Second)
		defer cancel2()
		gw.sessions.shutdownAll(destroyCtx)
		close(done)
	}()

	log.Printf("StreamKit web-capture gateway listening on %s -> %s", cfg.listenAddr, cfg.skitURL)
	if err := srv.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
		log.Fatalf("server error: %v", err)
	}
	<-done
}

// clampMin floors a capacity knob at 1: --max-concurrency < 1 would size
// clipSem as an unbuffered channel and deadlock every clip render, and
// --max-sessions/--max-viewers < 1 would silently reject every cast.
func clampMin(name string, n int) int {
	if n < 1 {
		log.Printf("invalid %s %d, using 1", name, n)
		return 1
	}
	return n
}

func loadConfig() config {
	listen := flag.String("listen", getEnvDefault("GATEWAY_LISTEN", defaultListenAddr), "Listen address")
	skit := flag.String("skit-url", getEnvDefault("SKIT_URL", defaultSkitURL), "Skit backend URL")
	token := flag.String("token", os.Getenv("SKIT_TOKEN"), "Bearer token for Skit (overrides SKIT_TOKEN env)")
	maxConc := flag.Int("max-concurrency", envInt("GATEWAY_MAX_CONCURRENCY", 4), "Max concurrent clip renders")
	clipDefault := flag.Int("clip-default-secs", envInt("GATEWAY_CLIP_DEFAULT_SECS", 10), "Default clip duration (seconds)")
	clipMax := flag.Int("clip-max-secs", envInt("GATEWAY_CLIP_MAX_SECS", 60), "Maximum clip duration (seconds)")
	maxSessions := flag.Int("max-sessions", envInt("GATEWAY_MAX_SESSIONS", 8), "Max concurrent live cast sessions")
	maxViewers := flag.Int("max-viewers", envInt("GATEWAY_MAX_VIEWERS", 10), "Max viewers per cast stream")
	idleSecs := flag.Int("session-idle-secs", envInt("GATEWAY_SESSION_IDLE_SECS", 30), "Idle grace before a viewerless session is reaped (seconds)")
	maxLifeSecs := flag.Int("session-max-secs", envInt("GATEWAY_SESSION_MAX_SECS", 1800), "Max lifetime of a cast session (seconds)")
	resolution := flag.String("resolution", getEnvDefault("GATEWAY_RESOLUTION", defaultResolution), "Capture resolution WxH (pages render and encode at this size, 1:1)")
	clipBitrate := flag.Int("clip-bitrate-kbps", envInt("GATEWAY_CLIP_BITRATE_KBPS", defaultClipBitrateKbps), "Clip video bitrate (kbps)")
	castBitrate := flag.Int("cast-bitrate-kbps", envInt("GATEWAY_CAST_BITRATE_KBPS", defaultCastBitrateKbps), "Cast video bitrate (kbps)")
	clipEncoder := flag.String("clip-encoder", getEnvDefault("GATEWAY_CLIP_ENCODER", defaultClipEncoder), "Clip encoder: h264-sw, h264-hw")
	castEncoder := flag.String("cast-encoder", getEnvDefault("GATEWAY_CAST_ENCODER", defaultCastEncoder), "Cast encoder: vp9-sw, av1-sw, av1-hw, h264-sw, h264-hw (h264 = fMP4, plays in Safari/iOS)")
	loadTimeout := flag.Int("load-timeout-secs", envInt("GATEWAY_LOAD_TIMEOUT_SECS", defaultLoadTimeoutSecs), "Max seconds the renderer holds the first frame waiting for page load (servo load_timeout_secs)")

	flag.Parse()

	clipDefaultDur := time.Duration(*clipDefault) * time.Second
	clipMaxDur := time.Duration(*clipMax) * time.Second
	if clipDefaultDur > clipMaxDur {
		clipDefaultDur = clipMaxDur
	}

	*maxConc = clampMin("--max-concurrency", *maxConc)
	*maxSessions = clampMin("--max-sessions", *maxSessions)
	*maxViewers = clampMin("--max-viewers", *maxViewers)
	*loadTimeout = clampMin("--load-timeout-secs", *loadTimeout)

	resW, resH, err := parseResolution(*resolution)
	if err != nil {
		log.Printf("invalid --resolution %q, using %s", *resolution, defaultResolution)
		resW, resH, _ = parseResolution(defaultResolution)
	}

	return config{
		listenAddr:     *listen,
		skitURL:        *skit,
		authToken:      *token,
		maxConcurrency: *maxConc,
		clipDefaultDur: clipDefaultDur,
		clipMaxDur:     clipMaxDur,
		resW:           resW,
		resH:           resH,
		clipBitrate:    *clipBitrate,
		castBitrate:    *castBitrate,
		clipEncoder:    *clipEncoder,
		castEncoder:    *castEncoder,
		loadTimeout:    *loadTimeout,
		maxSessions:    *maxSessions,
		maxViewers:     *maxViewers,
		idleTTL:        time.Duration(*idleSecs) * time.Second,
		maxLifetime:    time.Duration(*maxLifeSecs) * time.Second,
	}
}

func envInt(key string, def int) int {
	if v := os.Getenv(key); v != "" {
		if n, err := strconv.Atoi(v); err == nil && n > 0 {
			return n
		}
	}
	return def
}

func getEnvDefault(key, def string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return def
}

func (gw *gateway) router() http.Handler {
	metricsHandler := promhttp.Handler()
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/metrics":
			metricsHandler.ServeHTTP(w, r)
		case "/healthz", "/health":
			w.Header().Set("Content-Type", "text/plain")
			_, _ = io.WriteString(w, "ok\n")
		case "", "/", "/favicon.ico", "/robots.txt":
			gw.handleUsage(w, r)
		default:
			gw.handleCapture(w, r)
		}
	})
}

func (gw *gateway) handleCapture(w http.ResponseWriter, r *http.Request) {
	mode, rest, ok := detectMode(r.RequestURI)
	if !ok {
		gw.handleUsage(w, r)
		return
	}
	endpoint := mode.String()

	withMetrics(endpoint, w, r, func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet {
			http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
			return
		}
		def := captureOpts{dur: gw.clipDefaultDur, resW: gw.resW, resH: gw.resH}
		target, opts, err := parseTargetAndOptions(rest, def, gw.clipMaxDur)
		if err != nil {
			recordRejection(endpoint, reasonBadRequest)
			http.Error(w, "bad request: "+err.Error(), http.StatusBadRequest)
			return
		}
		// Only the DNS-free screen (scheme + literal-IP block) runs here; the
		// hostname DNS resolution is deferred into the render path so a browser
		// visit served the autoplay page pays no lookup (see serveClip/serveCast).
		u, err := parseTargetURL(target)
		if err != nil {
			recordRejection(endpoint, reasonSSRFBlocked)
			http.Error(w, "target host is not allowed", http.StatusForbidden)
			return
		}
		switch mode {
		case modeClip:
			gw.serveClip(w, r, u, opts)
		case modeCast:
			gw.serveCast(w, r, u, opts)
		}
	})
}

func (gw *gateway) serveClip(w http.ResponseWriter, r *http.Request, u *url.URL, opts captureOpts) {
	// A browser address-bar paste (Accept: text/html) gets an autoplay page; the
	// page's <video> then re-requests this same URL (Accept: */*) and is served
	// the raw MP4. Serving the page first avoids consuming a render slot for it.
	if acceptsHTML(r) {
		gw.writePlayerPage(w, r, "clip", u)
		return
	}
	if !gw.screenResolvable(w, r, u, "clip") {
		return
	}

	release, ok := gw.acquireClip(r.Context())
	if !ok {
		return // client disconnected while queued for a slot
	}
	defer release()

	yaml := renderClipPipeline(u.String(), opts.resW, opts.resH, captureFPS, gw.loadTimeout, clipFrames(opts.dur), gw.clipBitrate, gw.clipEnc)
	gw.proxyOneshot(w, r, yaml)
}

func (gw *gateway) serveCast(w http.ResponseWriter, r *http.Request, u *url.URL, opts captureOpts) {
	// A browser address-bar paste (Accept: text/html) gets an autoplay page; the
	// page's <video> then re-requests this same URL (Accept: */*) and is streamed.
	if acceptsHTML(r) {
		gw.writePlayerPage(w, r, "cast", u)
		return
	}
	if !gw.screenResolvable(w, r, u, "cast") {
		return
	}

	// Resolution is part of the render, so it keys the shared session: viewers of
	// the same URL+resolution share one pipeline; a different resolution is its
	// own. The YAML renders lazily — only a session-creating acquire uses it.
	key := fmt.Sprintf("%s|%dx%d", u.String(), opts.resW, opts.resH)
	s, err := gw.sessions.acquire(r.Context(), key, func() string {
		return renderCastPipeline(u.String(), opts.resW, opts.resH, captureFPS, gw.loadTimeout, gw.sessions.maxViewers, gw.castBitrate, gw.castEnc)
	})
	if err != nil {
		switch {
		case errors.Is(err, errOverCapacity):
			recordRejection("cast", reasonOverCapacity)
			http.Error(w, "server at capacity, try again shortly", http.StatusServiceUnavailable)
		case errors.Is(err, errShuttingDown):
			http.Error(w, "server shutting down", http.StatusServiceUnavailable)
		case errors.Is(err, context.Canceled):
			// client gave up before the stream was ready
		default:
			recordRejection("cast", reasonUpstreamError)
			http.Error(w, "failed to start stream", http.StatusBadGateway)
		}
		return
	}
	defer gw.sessions.release(s)
	gw.proxyMSE(w, r, s)
}

// screenResolvable completes SSRF validation for the render path by resolving
// the target hostname; it is called after the player-page short-circuit so a
// browser visit never pays the DNS lookup. Returns false (and writes a 403)
// when the host resolves to a blocked address.
func (gw *gateway) screenResolvable(w http.ResponseWriter, r *http.Request, u *url.URL, endpoint string) bool {
	if err := resolveTargetAllowed(r.Context(), u); err != nil {
		recordRejection(endpoint, reasonSSRFBlocked)
		http.Error(w, "target host is not allowed", http.StatusForbidden)
		return false
	}
	return true
}

// clipFrames rounds a duration to a whole number of frames (parseDuration
// accepts sub-second values, so truncation would skew the clip length).
func clipFrames(dur time.Duration) int {
	frames := int(math.Round(dur.Seconds() * float64(captureFPS)))
	return max(frames, 1)
}

// acquireClip takes a concurrency slot but bails if the client disconnects
// while queued — otherwise abandoned requests pile up on a full channel.
func (gw *gateway) acquireClip(ctx context.Context) (release func(), ok bool) {
	select {
	case gw.clipSem <- struct{}{}:
		return func() { <-gw.clipSem }, true
	case <-ctx.Done():
		return func() {}, false
	}
}
