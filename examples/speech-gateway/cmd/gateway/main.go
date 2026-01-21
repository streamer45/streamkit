// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Gateway translates simple STT/TTS requests into StreamKit oneshot multipart calls.
package main

import (
	"bytes"
	"context"
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
	client      *http.Client
	skitURL     string
	authToken   string
	maxBodySize int64
	sem         chan struct{}
}

type config struct {
	skitURL        string
	authToken      string
	listenAddr     string
	maxConcurrency int
	maxBodySize    int64
}

func main() {
	cfg := loadConfig()
	gw := &gateway{
		client:      newHTTPClient(),
		skitURL:     cfg.skitURL,
		authToken:   cfg.authToken,
		maxBodySize: cfg.maxBodySize,
		sem:         make(chan struct{}, cfg.maxConcurrency),
	}

	mux := http.NewServeMux()
	mux.HandleFunc("/stt", gw.handleSTT)
	mux.HandleFunc("/tts", gw.handleTTS)

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
	maxBody := flagInt64("max-body-bytes", envInt64("GATEWAY_MAX_BODY_BYTES", 10*1024*1024), "Maximum request body size")

	flag.Parse()

	return config{
		skitURL:        *skit,
		authToken:      *token,
		listenAddr:     *listen,
		maxConcurrency: *maxConc,
		maxBodySize:    *maxBody,
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
		http.Error(w, "Content-Type must be audio/ogg (Opus mono 48k)", http.StatusUnsupportedMediaType)
		return
	}
	release := gw.acquire()
	defer release()
	r.Body = http.MaxBytesReader(w, r.Body, gw.maxBodySize)
	useBuffer := r.ContentLength > 0 && r.ContentLength <= gw.maxBodySize
	if err := gw.proxyMultipart(w, r, sttPipelineYAML, "media", "audio/ogg", useBuffer); err != nil {
		log.Printf("stt error: %v", err)
		if !errors.Is(err, context.Canceled) {
			http.Error(w, "upstream error", http.StatusBadGateway)
		}
	}
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
		http.Error(w, "Content-Type must be text/plain", http.StatusUnsupportedMediaType)
		return
	}
	release := gw.acquire()
	defer release()
	r.Body = http.MaxBytesReader(w, r.Body, gw.maxBodySize)
	useBuffer := r.ContentLength > 0 && r.ContentLength <= gw.maxBodySize
	if err := gw.proxyMultipart(w, r, ttsPipelineYAML, "media", "text/plain", useBuffer); err != nil {
		log.Printf("tts error: %v", err)
		if !errors.Is(err, context.Canceled) {
			http.Error(w, "upstream error", http.StatusBadGateway)
		}
	}
}

func (gw *gateway) proxyMultipart(w http.ResponseWriter, r *http.Request, pipelineYAML, mediaField, mediaContentType string, bufferBody bool) error {
	ctx := r.Context()

	// Optionally buffer the request body for finite uploads (helps curl -T file).
	var src io.Reader = r.Body
	if bufferBody {
		limited := io.LimitReader(r.Body, gw.maxBodySize+1)
		buf, err := io.ReadAll(limited)
		if err != nil {
			return fmt.Errorf("buffer request body: %w", err)
		}
		if int64(len(buf)) > gw.maxBodySize {
			return fmt.Errorf("body too large")
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
		return fmt.Errorf("create skit request: %w", err)
	}
	req.Header.Set("Content-Type", mw.FormDataContentType())
	req.Close = true
	if gw.authToken != "" {
		req.Header.Set("Authorization", "Bearer "+gw.authToken)
	}

	resp, err := gw.client.Do(req)
	if err != nil {
		log.Printf("call skit failed: %v", err)
		return fmt.Errorf("call skit: %w", err)
	}
	defer func() {
		_ = resp.Body.Close()
	}()

	log.Printf("<- skit status=%d", resp.StatusCode)

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
		return nil
	}

	target := w.(io.Writer)
	if flusher != nil {
		target = flushWriter{w: w, f: flusher}
	}

	_, copyErr := io.Copy(target, resp.Body)
	if copyErr != nil {
		log.Printf("copy response error: %v", copyErr)
	}

	return copyErr
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
