// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

package main

import (
	"context"
	"errors"
	"fmt"
	"html"
	"io"
	"log"
	"mime/multipart"
	"net"
	"net/http"
	"net/url"
	"strings"
	"time"
)

// newHTTPClient builds a client with no overall timeout (oneshot renders take
// seconds; MSE streams run until the viewer leaves). keepAlives is off for the
// one-shot control/render calls so each gets a fresh connection, and on for the
// long-lived MSE proxy.
func newHTTPClient(keepAlives bool) *http.Client {
	return &http.Client{
		Transport: &http.Transport{
			DialContext: (&net.Dialer{
				Timeout:   5 * time.Second,
				KeepAlive: 30 * time.Second,
			}).DialContext,
			DisableKeepAlives: !keepAlives,
			ForceAttemptHTTP2: false,
		},
	}
}

func (gw *gateway) authReq(req *http.Request) {
	if gw.authToken != "" {
		req.Header.Set("Authorization", "Bearer "+gw.authToken)
	}
}

func (gw *gateway) failUpstream(w http.ResponseWriter, endpoint string, err error) {
	log.Printf("%s upstream error: %v", endpoint, err)
	recordRejection(endpoint, reasonUpstreamError)
	http.Error(w, "upstream error", http.StatusBadGateway)
}

// proxyOneshot runs a clip pipeline via POST /api/v1/process and streams the
// resulting MP4 back. The request body is a config-only multipart (the pipeline
// has no http_input, so no media part is needed).
func (gw *gateway) proxyOneshot(w http.ResponseWriter, r *http.Request, pipelineYAML string) {
	ctx := r.Context()

	bodyReader, bodyWriter := io.Pipe()
	mw := multipart.NewWriter(bodyWriter)
	go func() {
		defer func() { _ = bodyWriter.Close() }()
		field, err := mw.CreateFormField("config")
		if err != nil {
			bodyWriter.CloseWithError(err)
			return
		}
		if _, err := io.WriteString(field, strings.TrimSpace(pipelineYAML)); err != nil {
			bodyWriter.CloseWithError(err)
			return
		}
		if err := mw.Close(); err != nil {
			bodyWriter.CloseWithError(err)
		}
	}()

	req, err := http.NewRequestWithContext(ctx, http.MethodPost, gw.skitURL+"/api/v1/process", bodyReader)
	if err != nil {
		gw.failUpstream(w, "clip", fmt.Errorf("create skit request: %w", err))
		return
	}
	req.Header.Set("Content-Type", mw.FormDataContentType())
	req.Close = true
	gw.authReq(req)

	start := time.Now()
	resp, err := gw.client.Do(req)
	if err != nil {
		if errors.Is(err, context.Canceled) {
			return
		}
		gw.failUpstream(w, "clip", fmt.Errorf("call skit: %w", err))
		return
	}
	upstreamDuration.WithLabelValues("clip").Observe(time.Since(start).Seconds())
	defer func() { _ = resp.Body.Close() }()

	// A status outside [100,999] would panic w.WriteHeader below (Go rejects
	// such codes), so treat a malformed upstream status as a backend fault
	// before forwarding it.
	if resp.StatusCode < 100 || resp.StatusCode > 999 {
		gw.failUpstream(w, "clip", fmt.Errorf("invalid upstream status %d", resp.StatusCode))
		return
	}
	if resp.StatusCode >= 400 {
		slurp, _ := io.ReadAll(io.LimitReader(resp.Body, 4096))
		log.Printf("clip upstream status %d: %s", resp.StatusCode, strings.TrimSpace(string(slurp)))
		recordRejection("clip", reasonUpstreamError)
		http.Error(w, "failed to render page", http.StatusBadGateway)
		return
	}

	copyHeaders(w.Header(), resp.Header)
	if w.Header().Get("Content-Type") == "" {
		w.Header().Set("Content-Type", "video/mp4")
	}
	w.WriteHeader(resp.StatusCode)
	streamCopy(ctx, w, resp.Body, "clip")
}

// proxyMSE streams a live WebM session to the viewer. The http_mse path only
// registers once the session's pipeline starts, so a freshly created session
// may 404 briefly — retry until ready or the viewer goes away.
func (gw *gateway) proxyMSE(w http.ResponseWriter, r *http.Request, s *liveSession) {
	ctx := r.Context()
	streamURL := gw.skit.streamURL(s.id)
	deadline := time.Now().Add(gw.mseReadyTimeout)
	retry := time.NewTicker(200 * time.Millisecond)
	defer retry.Stop()

	var resp *http.Response
	for {
		req, err := http.NewRequestWithContext(ctx, http.MethodGet, streamURL, nil)
		if err != nil {
			gw.failUpstream(w, "cast", err)
			return
		}
		gw.authReq(req)
		r2, err := gw.streamClient.Do(req)
		if err == nil && r2.StatusCode == http.StatusOK {
			resp = r2
			break
		}
		if r2 != nil {
			_, _ = io.Copy(io.Discard, io.LimitReader(r2.Body, 2048))
			_ = r2.Body.Close()
		}
		if ctx.Err() != nil {
			return
		}
		if time.Now().After(deadline) {
			if err != nil {
				gw.failUpstream(w, "cast", err)
			} else {
				gw.failUpstream(w, "cast", fmt.Errorf("mse stream not ready (status %d)", r2.StatusCode))
			}
			return
		}
		select {
		case <-ctx.Done():
			return
		case <-retry.C:
		}
	}
	defer func() { _ = resp.Body.Close() }()

	// Close the upstream body the instant the viewer's connection ends, so
	// streamCopy's read unblocks immediately. Without this, a read parked
	// between frames keeps the session's viewer count > 0 and dodges idle
	// reaping until skit happens to write again. ctx is also canceled when the
	// handler returns, so this goroutine never outlives the request.
	go func() {
		<-ctx.Done()
		_ = resp.Body.Close()
	}()

	copyHeaders(w.Header(), resp.Header)
	if w.Header().Get("Content-Type") == "" {
		w.Header().Set("Content-Type", "video/webm")
	}
	w.WriteHeader(http.StatusOK)
	streamCopy(ctx, w, resp.Body, "cast")
}

// streamCopy forwards an upstream body to the client, flushing each chunk so
// live/progressive playback starts without waiting for the whole response.
func streamCopy(ctx context.Context, w http.ResponseWriter, src io.Reader, endpoint string) {
	var dst io.Writer = w
	if flusher, ok := w.(http.Flusher); ok {
		dst = flushWriter{w: w, f: flusher}
	}
	// A client disconnect (ctx canceled, which also closes the upstream body) is
	// the normal way a live stream ends; only log a copy error that happened
	// while the client was still connected.
	if _, err := io.Copy(dst, src); err != nil && ctx.Err() == nil {
		log.Printf("%s copy response error: %v", endpoint, err)
	}
}

const castPageTemplate = `<!doctype html><html><head><meta charset="utf-8">` +
	`<title>web-cast · %s</title>` +
	`<style>html,body{margin:0;height:100%%;background:#000}video{width:100%%;height:100%%;object-fit:contain}</style>` +
	`</head><body><video src="%s" autoplay muted playsinline controls></video></body></html>`

func (gw *gateway) writeCastPage(w http.ResponseWriter, r *http.Request, target *url.URL) {
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	w.WriteHeader(http.StatusOK)
	_, _ = fmt.Fprintf(w, castPageTemplate, html.EscapeString(target.String()), html.EscapeString(r.RequestURI))
}

const usagePage = `<!doctype html><html><head><meta charset="utf-8"><title>StreamKit web-capture</title>` +
	`<style>body{font:16px/1.5 system-ui,sans-serif;max-width:42rem;margin:3rem auto;padding:0 1rem;color:#222}` +
	`code{background:#f2f2f2;padding:.1em .3em;border-radius:.2em}</style></head><body>` +
	`<h1>StreamKit web-capture</h1>` +
	`<p>Render any web page to video. Paste a target URL straight after the host:</p>` +
	`<ul>` +
	`<li><strong>Clip</strong> (MP4 file): <code>clip.streamkit.dev/example.com</code></li>` +
	`<li><strong>Clip</strong> with duration: <code>clip.streamkit.dev/dur=30s/example.com</code></li>` +
	`<li><strong>Cast</strong> (live stream): <code>cast.streamkit.dev/example.com</code></li>` +
	`</ul>` +
	`<p>Locally, use a path prefix instead of a subdomain: <code>/clip/…</code> or <code>/cast/…</code>.</p>` +
	`</body></html>`

func (gw *gateway) handleUsage(w http.ResponseWriter, _ *http.Request) {
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	_, _ = io.WriteString(w, usagePage)
}

func acceptsHTML(r *http.Request) bool {
	return strings.Contains(r.Header.Get("Accept"), "text/html")
}

// hopByHop headers are connection-scoped and must not be forwarded by a proxy
// (RFC 7230 §6.1); Content-Length is also dropped since Go sets it from the
// streamed body.
var hopByHop = map[string]bool{
	"connection":          true,
	"keep-alive":          true,
	"proxy-authenticate":  true,
	"proxy-authorization": true,
	"te":                  true,
	"trailer":             true,
	"transfer-encoding":   true,
	"upgrade":             true,
	"content-length":      true,
}

func copyHeaders(dst, src http.Header) {
	for k, vv := range src {
		if hopByHop[strings.ToLower(k)] {
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

func logging(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		start := time.Now()
		next.ServeHTTP(w, r)
		log.Printf("%s %s remote=%s %s", r.Method, r.RequestURI, r.RemoteAddr, time.Since(start).Truncate(time.Millisecond))
	})
}
