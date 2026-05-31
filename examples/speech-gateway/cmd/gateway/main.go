// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Gateway translates simple STT/TTS requests into StreamKit oneshot multipart calls.
package main

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"log"
	"mime/multipart"
	"net"
	"net/http"
	"net/textproto"
	"os"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/prometheus/client_golang/prometheus/promhttp"
)

const (
	defaultSkitURL    = "http://127.0.0.1:4545"
	defaultListenAddr = ":8080"

	sttPipelineYAML = `
name: stt-ogg-opus
description: STT over streamed Ogg/Opus
mode: oneshot
steps:
  - kind: streamkit::http_input

  - kind: containers::ogg::demuxer

  - kind: audio::opus::decoder

  - kind: audio::resampler
    params:
      chunk_frames: 960
      output_frame_size: 960
      target_sample_rate: 16000

  - kind: plugin::native::whisper
    params:
      model_path: models/ggml-base.en-q5_1.bin
      language: en
      vad_model_path: models/silero_vad.onnx
      vad_threshold: 0.5
      min_silence_duration_ms: 700
      max_segment_duration_secs: 30.0

  - kind: core::json_serialize
    params:
      pretty: false
      newline_delimited: true

  - kind: streamkit::http_output
    params:
      content_type: application/json
`

	ttsPipelineYAML = `
name: tts-ogg-opus
description: TTS to streamed Ogg/Opus
mode: oneshot
steps:
  - kind: streamkit::http_input
  - kind: core::text_chunker
    params:
      min_length: 10
  - kind: plugin::native::kokoro
    params:
      model_dir: "models/kokoro-multi-lang-v1_1"
      speaker_id: 0
      speed: 1.0
      num_threads: 4
  - kind: audio::resampler
    params:
      chunk_frames: 960
      output_frame_size: 960
      target_sample_rate: 48000
  - kind: audio::opus::encoder
  - kind: containers::ogg::muxer
    params:
      channels: 1
      codec: opus
      chunk_size: 32768
  - kind: streamkit::http_output
    params:
      content_type: audio/ogg
`
)

type gateway struct {
	client         *http.Client
	skitURL        string
	authToken      string
	maxBodySize    int64
	maxTTSTextSize int64
	sem            chan struct{}
}

type config struct {
	skitURL        string
	authToken      string
	listenAddr     string
	maxConcurrency int
	maxBodySize    int64
	maxTTSTextSize int64
}

func main() {
	cfg := loadConfig()
	gw := &gateway{
		client:         newHTTPClient(),
		skitURL:        cfg.skitURL,
		authToken:      cfg.authToken,
		maxBodySize:    cfg.maxBodySize,
		maxTTSTextSize: cfg.maxTTSTextSize,
		sem:            make(chan struct{}, cfg.maxConcurrency),
	}

	mux := http.NewServeMux()
	mux.HandleFunc("/stt", instrument("stt", gw.handleSTT))
	mux.HandleFunc("/tts", instrument("tts", gw.handleTTS))
	// /metrics is intentionally not gated by the concurrency semaphore so it
	// stays scrapable while request slots are saturated.
	mux.Handle("/metrics", promhttp.Handler())

	server := &http.Server{
		Addr:              cfg.listenAddr,
		Handler:           logging(mux),
		ReadHeaderTimeout: 5 * time.Second,
	}

	log.Printf("StreamKit speech gateway listening on %s -> %s", cfg.listenAddr, cfg.skitURL)
	if err := server.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
		log.Fatalf("server error: %v", err)
	}
}

func loadConfig() config {
	listen := flagString("listen", getEnvDefault("GATEWAY_LISTEN", defaultListenAddr), "Listen address for the gateway")
	skit := flagString("skit-url", getEnvDefault("SKIT_URL", defaultSkitURL), "Skit backend URL")
	token := flagString("token", os.Getenv("SKIT_TOKEN"), "Bearer token for Skit (overrides SKIT_TOKEN env)")
	maxConc := flagInt("max-concurrency", envInt("GATEWAY_MAX_CONCURRENCY", 10), "Maximum concurrent in-flight requests")
	maxBody := flagInt64("max-body-bytes", envInt64("GATEWAY_MAX_BODY_BYTES", 1*1024*1024), "Maximum request body size")
	maxTTSText := flagInt64("max-tts-text-size", envInt64("GATEWAY_MAX_TTS_TEXT_SIZE", 1000), "Maximum TTS text size in characters")

	flag.Parse()

	return config{
		skitURL:        *skit,
		authToken:      *token,
		listenAddr:     *listen,
		maxConcurrency: *maxConc,
		maxBodySize:    *maxBody,
		maxTTSTextSize: *maxTTSText,
	}
}

func flagString(name, def, usage string) *string {
	return flag.String(name, def, usage)
}

func flagInt(name string, def int, usage string) *int {
	return flag.Int(name, def, usage)
}

func flagInt64(name string, def int64, usage string) *int64 {
	return flag.Int64(name, def, usage)
}

func envInt(key string, def int) int {
	if v := os.Getenv(key); v != "" {
		if n, err := strconv.Atoi(v); err == nil && n > 0 {
			return n
		}
	}
	return def
}

func envInt64(key string, def int64) int64 {
	if v := os.Getenv(key); v != "" {
		if n, err := strconv.ParseInt(v, 10, 64); err == nil && n > 0 {
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

func newHTTPClient() *http.Client {
	tr := &http.Transport{
		DialContext: (&net.Dialer{
			Timeout:   5 * time.Second,
			KeepAlive: 30 * time.Second,
		}).DialContext,
		MaxIdleConns:          0,
		MaxConnsPerHost:       0,
		MaxIdleConnsPerHost:   0,
		IdleConnTimeout:       0,
		DisableKeepAlives:     true,
		ExpectContinueTimeout: 0,
		ForceAttemptHTTP2:     false,
	}
	return &http.Client{Transport: tr}
}

func (gw *gateway) acquire() func() {
	gw.sem <- struct{}{}
	return func() { <-gw.sem }
}

func (gw *gateway) handleSTT(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost && r.Method != http.MethodPut {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	ct := r.Header.Get("Content-Type")
	switch strings.ToLower(ct) {
	case "", "application/octet-stream", "application/x-www-form-urlencoded", "text/plain":
		ct = "audio/ogg"
		log.Printf("stt request missing/unknown Content-Type, assuming %s", ct)
	}
	log.Printf("stt request start remote=%s ct=%s", r.RemoteAddr, ct)
	defer func() {
		_ = r.Body.Close()
	}()
	if !strings.HasPrefix(ct, "audio/ogg") {
		log.Printf("stt unsupported content type: %s", ct)
		recordRejection("stt", reasonBadContentType)
		http.Error(w, "Content-Type must be audio/ogg (Opus mono 48k)", http.StatusUnsupportedMediaType)
		return
	}
	if r.ContentLength > gw.maxBodySize {
		log.Printf("stt body too large: %d bytes (max: %d)", r.ContentLength, gw.maxBodySize)
		recordRejection("stt", reasonTooLarge)
		http.Error(w, "request body too large", http.StatusRequestEntityTooLarge)
		return
	}
	release := gw.acquire()
	defer release()
	r.Body = http.MaxBytesReader(underlying(w), r.Body, gw.maxBodySize)
	useBuffer := r.ContentLength > 0 && r.ContentLength <= gw.maxBodySize
	gw.proxyMultipart(w, r, "stt", sttPipelineYAML, "media", "audio/ogg", useBuffer)
}

func (gw *gateway) handleTTS(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost && r.Method != http.MethodPut {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	ct := r.Header.Get("Content-Type")
	switch strings.ToLower(ct) {
	case "", "application/octet-stream", "application/x-www-form-urlencoded":
		ct = "text/plain"
		log.Printf("tts request missing/unknown Content-Type, assuming %s", ct)
	}
	log.Printf("tts request start remote=%s ct=%s", r.RemoteAddr, ct)
	defer func() {
		_ = r.Body.Close()
	}()
	if !strings.HasPrefix(ct, "text/plain") {
		recordRejection("tts", reasonBadContentType)
		http.Error(w, "Content-Type must be text/plain", http.StatusUnsupportedMediaType)
		return
	}
	if r.ContentLength > gw.maxBodySize {
		log.Printf("tts body too large: %d bytes (max: %d)", r.ContentLength, gw.maxBodySize)
		recordRejection("tts", reasonTooLarge)
		http.Error(w, "request body too large", http.StatusRequestEntityTooLarge)
		return
	}
	release := gw.acquire()
	defer release()

	// Read and validate text size
	r.Body = http.MaxBytesReader(underlying(w), r.Body, gw.maxBodySize)

	// UTF-8 characters can be up to 4 bytes, so read up to 4x the character limit
	// to ensure we can properly count characters and detect if input exceeds limit
	maxReadBytes := gw.maxTTSTextSize * 4
	textBytes, err := io.ReadAll(io.LimitReader(r.Body, maxReadBytes))
	if err != nil {
		log.Printf("tts read error: %v", err)
		http.Error(w, "failed to read request body", http.StatusBadRequest)
		return
	}

	// Count UTF-8 runes (characters) instead of bytes
	runeCount := int64(utf8.RuneCount(textBytes))

	// If we read the full buffer, check if there's more data
	if int64(len(textBytes)) == maxReadBytes {
		// Try to read one more byte to see if there's more
		extra := make([]byte, 1)
		n, _ := r.Body.Read(extra)
		if n > 0 {
			// There's more data, so we definitely exceeded the limit
			log.Printf("tts text too large: >%d chars (max: %d)", runeCount, gw.maxTTSTextSize)
			recordRejection("tts", reasonTooLarge)
			http.Error(w, fmt.Sprintf("text too large: exceeds %d characters", gw.maxTTSTextSize), http.StatusRequestEntityTooLarge)
			return
		}
	}

	if runeCount > gw.maxTTSTextSize {
		log.Printf("tts text too large: %d chars (max: %d)", runeCount, gw.maxTTSTextSize)
		recordRejection("tts", reasonTooLarge)
		http.Error(w, fmt.Sprintf("text too large: %d characters (max: %d)", runeCount, gw.maxTTSTextSize), http.StatusRequestEntityTooLarge)
		return
	}

	log.Printf("tts text length: %d chars (%d bytes)", runeCount, len(textBytes))

	// Replace body with buffered content
	r.Body = io.NopCloser(bytes.NewReader(textBytes))

	useBuffer := true // We've already buffered it
	gw.proxyMultipart(w, r, "tts", ttsPipelineYAML, "media", "text/plain", useBuffer)
}

// failUpstream is the single place a gateway-side upstream failure is reported,
// so the rejection counter and the 502 status never diverge.
func (gw *gateway) failUpstream(w http.ResponseWriter, endpoint string, err error) {
	log.Printf("%s upstream error: %v", endpoint, err)
	recordRejection(endpoint, reasonUpstreamError)
	http.Error(w, "upstream error", http.StatusBadGateway)
}

// underlying reaches the writer wrapped by instrument so http.MaxBytesReader can
// force-close the connection on overflow (the embedded interface hides it).
func underlying(w http.ResponseWriter) http.ResponseWriter {
	if u, ok := w.(interface{ Unwrap() http.ResponseWriter }); ok {
		return u.Unwrap()
	}
	return w
}

// proxyMultipart owns the full response for an STT/TTS request: it forwards the
// upstream result, and classifies any gateway-side failure to a status/rejection
// reason itself. Once it has committed response headers (200 streaming begins),
// a mid-stream failure can no longer be relabeled, so it is only logged.
func (gw *gateway) proxyMultipart(w http.ResponseWriter, r *http.Request, endpoint, pipelineYAML, mediaField, mediaContentType string, bufferBody bool) {
	ctx := r.Context()

	// Optionally buffer the request body for finite uploads (helps curl -T file).
	var src io.Reader = r.Body
	if bufferBody {
		// r.Body is already MaxBytesReader-bounded by the handler, so an oversize
		// read surfaces as *http.MaxBytesError rather than an unbounded buffer.
		buf, err := io.ReadAll(r.Body)
		if err != nil {
			var maxErr *http.MaxBytesError
			if errors.As(err, &maxErr) {
				log.Printf("%s body too large", endpoint)
				recordRejection(endpoint, reasonTooLarge)
				http.Error(w, "request body too large", http.StatusRequestEntityTooLarge)
				return
			}
			gw.failUpstream(w, endpoint, fmt.Errorf("buffer request body: %w", err))
			return
		}
		log.Printf("buffered upload (%d bytes) before forwarding", len(buf))
		src = bytes.NewReader(buf)
	}

	bodyReader, bodyWriter := io.Pipe()
	mw := multipart.NewWriter(bodyWriter)

	// writer goroutine
	go func() {
		defer func() {
			_ = bodyWriter.Close()
		}()
		log.Printf("-> skit /api/v1/process (pipeline=%s, field=%s)", pipelineName(pipelineYAML), mediaField)
		if err := writeConfigPart(mw, pipelineYAML); err != nil {
			log.Printf("multipart config error: %v", err)
			bodyWriter.CloseWithError(err)
			return
		}
		if err := writeStreamPart(mw, mediaField, mediaContentType, src); err != nil {
			log.Printf("multipart stream error: %v", err)
			bodyWriter.CloseWithError(err)
			return
		}
		if err := mw.Close(); err != nil {
			log.Printf("multipart close error: %v", err)
			bodyWriter.CloseWithError(err)
		}
	}()

	req, err := http.NewRequestWithContext(ctx, http.MethodPost, gw.skitURL+"/api/v1/process", bodyReader)
	if err != nil {
		gw.failUpstream(w, endpoint, fmt.Errorf("create skit request: %w", err))
		return
	}
	req.Header.Set("Content-Type", mw.FormDataContentType())
	req.Close = true
	// Consumed by the backend's oneshot metrics (sibling PR #545) to split
	// oneshot_pipeline.duration by service {tts,stt,other}.
	req.Header.Set("X-StreamKit-Service", endpoint)
	if gw.authToken != "" {
		req.Header.Set("Authorization", "Bearer "+gw.authToken)
	}

	upstreamStart := time.Now()
	resp, err := gw.client.Do(req)
	if err != nil {
		// An oversize body trips MaxBytesReader in the writer goroutine and
		// surfaces here; that is a client size violation, not a backend fault.
		var maxErr *http.MaxBytesError
		if errors.As(err, &maxErr) {
			log.Printf("%s body too large during stream", endpoint)
			recordRejection(endpoint, reasonTooLarge)
			http.Error(w, "request body too large", http.StatusRequestEntityTooLarge)
			return
		}
		if errors.Is(err, context.Canceled) {
			log.Printf("%s client canceled before upstream response", endpoint)
			return
		}
		gw.failUpstream(w, endpoint, fmt.Errorf("call skit: %w", err))
		return
	}
	// Record only requests that actually received response headers; dial timeouts
	// and cancellations above never reached the backend.
	upstreamDuration.WithLabelValues(endpoint).Observe(time.Since(upstreamStart).Seconds())
	defer func() {
		_ = resp.Body.Close()
	}()

	log.Printf("<- skit status=%d", resp.StatusCode)

	// net/http panics in WriteHeader for codes outside [100,999]; a malformed
	// upstream status line is a backend fault, handled before headers commit.
	if resp.StatusCode < 100 || resp.StatusCode > 999 {
		gw.failUpstream(w, endpoint, fmt.Errorf("invalid upstream status %d", resp.StatusCode))
		return
	}

	copyHeaders(w.Header(), resp.Header)
	// Avoid forwarding length/transfer headers so Go can stream-chunk the proxied body.
	w.Header().Del("Content-Length")
	w.Header().Del("Transfer-Encoding")
	w.WriteHeader(resp.StatusCode)

	flusher, _ := w.(http.Flusher)

	if resp.StatusCode >= 400 {
		slurp, _ := io.ReadAll(io.LimitReader(resp.Body, 4096))
		if len(slurp) > 0 {
			log.Printf("upstream error body: %s", strings.TrimSpace(string(slurp)))
		}
		if _, err := w.Write(slurp); err != nil {
			log.Printf("write error: %v", err)
		}
		return
	}

	// STT transcriptions arrive as newline-delimited, externally-tagged Packet
	// JSON ({"Transcription": {...}}). Unwrap and flush each line as it arrives so
	// streaming is preserved and memory stays bounded to a single line. Audio
	// (TTS) is non-JSON and copied through unchanged.
	if isJSONContentType(resp.Header.Get("Content-Type")) {
		br := bufio.NewReader(resp.Body)
		for {
			line, err := br.ReadBytes('\n')
			if len(line) > 0 {
				if _, werr := w.Write(unwrapPacketJSON(line)); werr != nil {
					log.Printf("%s write error: %v", endpoint, werr)
					return
				}
				if flusher != nil {
					flusher.Flush()
				}
			}
			if err != nil {
				if !errors.Is(err, io.EOF) && !errors.Is(err, context.Canceled) {
					log.Printf("%s read response error: %v", endpoint, err)
				}
				return
			}
		}
	}

	target := w.(io.Writer)
	if flusher != nil {
		target = flushWriter{w: w, f: flusher}
	}

	// The 200 status line is already on the wire, so a failure here cannot be
	// turned into a 502; log it without double-counting a gateway rejection.
	if _, copyErr := io.Copy(target, resp.Body); copyErr != nil && !errors.Is(copyErr, context.Canceled) {
		log.Printf("%s copy response error: %v", endpoint, copyErr)
	}
}

func writeConfigPart(mw *multipart.Writer, pipelineYAML string) error {
	part, err := mw.CreateFormField("config")
	if err != nil {
		return fmt.Errorf("create config part: %w", err)
	}
	if _, err := io.WriteString(part, strings.TrimSpace(pipelineYAML)); err != nil {
		return fmt.Errorf("write config: %w", err)
	}
	return nil
}

func writeStreamPart(mw *multipart.Writer, fieldName, contentType string, src io.Reader) error {
	h := textproto.MIMEHeader{}
	h.Set("Content-Disposition", fmt.Sprintf(`form-data; name="%s"; filename="media"`, fieldName))
	h.Set("Content-Type", contentType)

	part, err := mw.CreatePart(h)
	if err != nil {
		return fmt.Errorf("create media part: %w", err)
	}
	if n, err := io.Copy(part, src); err != nil {
		if errors.Is(err, io.EOF) || errors.Is(err, io.ErrUnexpectedEOF) {
			log.Printf("media stream ended early after %d bytes: %v", n, err)
			return nil
		}
		log.Printf("media copy error after %d bytes: %v", n, err)
		return fmt.Errorf("copy media: %w", err)
	}
	return nil
}

func isJSONContentType(ct string) bool {
	mediaType := ct
	if i := strings.IndexByte(mediaType, ';'); i >= 0 {
		mediaType = mediaType[:i]
	}
	return strings.EqualFold(strings.TrimSpace(mediaType), "application/json")
}

// packet names the StreamKit Packet variant the gateway flattens. The inner
// value is kept as RawMessage and forwarded verbatim, so the gateway strips only
// the outer wrapper without coupling to the transcription schema.
type packet struct {
	Transcription json.RawMessage `json:"Transcription"`
}

// unwrapPacketJSON flattens the STT Packet enum ({"Transcription": {...}}) to the
// inner transcription object. The backend emits newline-delimited JSON, so each
// line is handled independently; lines that are not a Transcription packet are
// left unchanged.
func unwrapPacketJSON(body []byte) []byte {
	lines := bytes.Split(body, []byte("\n"))
	changed := false
	for i, line := range lines {
		if inner, ok := unwrapTranscription(bytes.TrimSpace(line)); ok {
			lines[i] = inner
			changed = true
		}
	}
	if !changed {
		return body
	}
	return bytes.Join(lines, []byte("\n"))
}

func unwrapTranscription(line []byte) ([]byte, bool) {
	var pkt packet
	if err := json.Unmarshal(line, &pkt); err != nil {
		return nil, false
	}
	inner := bytes.TrimSpace(pkt.Transcription)
	if len(inner) == 0 || inner[0] != '{' {
		return nil, false
	}
	return inner, true
}

func copyHeaders(dst, src http.Header) {
	for k, vv := range src {
		if strings.EqualFold(k, "Content-Length") || strings.EqualFold(k, "Transfer-Encoding") {
			continue
		}
		for _, v := range vv {
			dst.Add(k, v)
		}
	}
}

type flushWriter struct {
	w io.Writer
	f http.Flusher
}

func (fw flushWriter) Write(p []byte) (int, error) {
	n, err := fw.w.Write(p)
	if fw.f != nil {
		fw.f.Flush()
	}
	return n, err
}

// logging middleware (minimal)
func logging(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		log.Printf("recv %s %s remote=%s ct=%s", r.Method, r.URL.Path, r.RemoteAddr, r.Header.Get("Content-Type"))
		start := time.Now()
		next.ServeHTTP(w, r)
		log.Printf("%s %s %s", r.Method, r.URL.Path, time.Since(start).Truncate(time.Millisecond))
	})
}

func pipelineName(yaml string) string {
	for _, line := range strings.Split(yaml, "\n") {
		line = strings.TrimSpace(line)
		if strings.HasPrefix(line, "name:") {
			return strings.TrimSpace(strings.TrimPrefix(line, "name:"))
		}
	}
	return "unknown"
}
