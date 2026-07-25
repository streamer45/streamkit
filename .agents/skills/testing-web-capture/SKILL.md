---
name: testing-web-capture
description: End-to-end testing of the StreamKit web-capture gateway demo (examples/web-capture) with the native servo plugin. Use when verifying clip/cast endpoints, servo page rendering, or cross-session pixel isolation.
---

# Testing the web-capture gateway demo

## Setup
1. Go toolchain is required for the gateway (`/usr/local/go/bin/go`); install from go.dev tarball if missing.
2. Build + deploy the servo plugin: `just build-plugin-native-servo && just copy-plugins-native`.
   - Verify freshness: `md5sum target/plugins/release/libservo_web.so .plugins/native/servo/libservo_web.so` must match and have a recent timestamp. Stale `.so` files are a known source of white frames / cross-request corruption.
   - skit loads plugins only at startup — restart `just skit` after copying.
3. Start backend: `just skit` (127.0.0.1:4545; auth Auto disables auth on loopback, no token needed locally).
4. Start gateway: `cd examples/web-capture && go run ./cmd/gateway --listen :8080 --skit-url http://127.0.0.1:4545`.

## Endpoints
- Finite clip: `http://127.0.0.1:8080/clip/dur=20s,res=1280x720/<target-url>` → MP4 (h264-sw).
- Live cast: `http://127.0.0.1:8080/cast/<target-url>` → WebM (vp9-sw). curl with `-H 'Accept: video/webm'` to grab raw stream.
- SSRF guard blocks loopback/private targets — use public sites (streamkit.dev is dark-themed, example.com light-themed: a good high-contrast pair for leak checks).

## Verifying frames (don't trust playback alone)
- Extract frames: `ffmpeg -i clip.mp4 -vf "select='eq(n,30)+eq(n,300)'" -vsync 0 f_%d.png`
- Pixel stats with PIL: min/max grey and dark-pixel fraction distinguish blank white (min≈255), blank black/pre-paint (max≈0), and real content.
- Since the first-load gate (plugin >= 0.2.2), clips/casts should show page content from frame 0 (the node holds emission until loaded+painted, capped by `load_timeout_secs`, default 30s). Pre-0.2.2 builds may show all-black cold-start clips or blank lead-ins.
- Some pages (e.g. streamkit.dev) may still show a single white first frame — the page's own pre-theme paint, not a gate failure.
- Testing the load-timeout expiry path via the gateway is hard: SSRF blocks local hanging servers and public "slow" endpoints (httpstat.us sleep, closed ports) fail fast rather than hang.

## Cross-session leak checks
- Sequential: capture page A then page B; sampled B frames must contain no A pixels (use dark/light pages so pixel stats catch leaks).
- Concurrent: run a cast of A and a clip of B simultaneously (two curls with `&`); sample start/middle/end frames of both.
- Regression test: `cd plugins/native/servo && CARGO_TARGET_DIR=../../../target/plugins cargo test --release --test cross_session_leak` (heavy; llvmpipe; pass = exit 0).

## Devin Secrets Needed
None for local loopback testing (auth Auto disables auth).
