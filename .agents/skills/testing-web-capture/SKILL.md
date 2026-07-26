---
name: testing-web-capture
description: >-
  End-to-end testing of the StreamKit web-capture gateway demo
  (examples/web-capture) with the native servo plugin. Use when verifying
  clip/cast endpoints, servo page rendering, or cross-session pixel isolation.
license: MPL-2.0
---

# Testing the web-capture gateway demo

## Setup
1. Go toolchain is required for the gateway (`/usr/local/go/bin/go`); install from go.dev tarball if missing.
2. Build + deploy the servo plugin: `just build-plugin-native-servo && just copy-plugins-native`.
   - Verify freshness: `md5sum target/plugins/release/libservo_web.so .plugins/native/servo/libservo_web.so` must match and have a recent timestamp. Stale `.so` files are a known source of white frames / cross-request corruption.
   - skit loads plugins only at startup — restart `just skit` after copying.
3. Start backend: `just skit` (127.0.0.1:4545; auth Auto disables auth on loopback, no token needed locally).
4. Start gateway: `cd examples/web-capture && go run ./cmd/gateway --listen :8080 --skit-url http://127.0.0.1:4545`.
   - `--load-timeout-secs N` / `GATEWAY_LOAD_TIMEOUT_SECS` (default 5) sets servo's `load_timeout_secs` in both
     pipeline templates. It must stay below the gateway's hard-coded 8s `mseReadyTimeout`, or cast viewers 503.
   - To test a non-default value, run a second gateway on another port (e.g. `--listen :8081 --load-timeout-secs 2`);
     both can share one skit.

## Endpoints
- Finite clip: `http://127.0.0.1:8080/clip/dur=20s,res=1280x720/<target-url>` → MP4 (h264-sw).
- Live cast: `http://127.0.0.1:8080/cast/<target-url>` → WebM (vp9-sw). curl with `-H 'Accept: video/webm'` to grab raw stream.
- SSRF guard blocks loopback/private targets — use public sites (streamkit.dev is dark-themed, example.com light-themed: a good high-contrast pair for leak checks).

## Verifying frames (don't trust playback alone)
- Extract frames: `ffmpeg -i clip.mp4 -vf "select='eq(n,30)+eq(n,300)'" -vsync 0 f_%d.png`
- Live cast WebM files have no cues — `ffmpeg -ss`/`-sseof` seeking silently returns the first frame. Extract frames
  with `select='eq(n,N)'` full decodes instead, and beware lexicographic sorting of `%d`-numbered frame files.
- Pixel stats with PIL: min/max grey and dark-pixel fraction distinguish blank white (min≈255), blank black/pre-paint (max≈0), and real content.
- Since the first-load gate (plugin >= 0.2.2), clips/casts should show page content from frame 0 (the node holds emission until the page is ready, capped by `load_timeout_secs`, default 30s). Pre-0.2.2 builds may show all-black cold-start clips or blank lead-ins.
- Since plugin >= 0.2.3, "ready" is load-complete OR ~2s after first paint — ad-heavy pages that never fire their load event no longer stall the full timeout. The gateway also caps `load_timeout_secs` at 5s (GATEWAY_LOAD_TIMEOUT_SECS), so worst-case time-to-first-byte through the gateway is ~5s plus one GOP (~1s).
- Some pages (e.g. streamkit.dev) may still show a single white first frame — the page's own pre-theme paint, not a gate failure.
- Testing the load-timeout expiry path via the gateway is hard: SSRF blocks local hanging servers and public "slow" endpoints (httpstat.us sleep, closed ports) fail fast rather than hang. In practice pages release via load-complete or the post-paint branch, so the expiry warn may never fire — say so rather than claiming that branch was covered.
- The gate branch that fired is visible in the skit log (plugin logs, `grep emission /tmp/skit.log`):
  `Initial page loaded; starting frame emission` (load-complete), `Initial page painted but load still pending after 2s; …`
  (the >=0.2.3 post-paint branch — expect this on ad-heavy pages like cnn.com/theverge.com, ~2.2s after servo init),
  or `Page did not finish loading within Ns; …` (timeout cap).

## Verifying which params actually reached the pipeline
- Cast sessions: the gateway logs `cast: session <id> created (key="<url>|WxH")`; then
  `curl http://127.0.0.1:4545/api/v1/sessions/<id>/pipeline` returns the deployed graph JSON — grep the `web` node's
  params for `load_timeout_secs`, `fps`, `width`/`height`. `GET /api/v1/sessions` lists live session ids.
- Cast dedupe/refcount evidence is in the gateway log: one `session … created` line per `url|WxH` key plus
  `viewer left … (viewers=N)` on disconnect. Note the gateway logs a request only when it *completes*, so
  still-streaming viewers do not appear yet — cross-check with `GET /api/v1/sessions` (count must stay 1).

## Browser playback checks
- Pointing the browser address bar at `/cast/<url>` does serve the gateway autoplay player page, but the
  browser-automation tool times out on navigate/view because a live stream keeps the page `load` event pending.
  Workaround: serve a tiny local page (`python3 -m http.server 8090`) whose `<video src>` is the gateway URL, plus a
  `setInterval` readout of `readyState/currentTime/buffered/paused/error`. The media path is identical and the page
  load completes, so screenshots work — and the on-screen `currentTime` is proof playback advances.
- Autoplay may land paused in this environment: click the play control, then compare `currentTime` across two views.
- A cast joined mid-session starts at a non-zero `currentTime` (live stream) — not a defect.

## Tuning a live cast's URL
- Build the CLI with `cargo build --release -p streamkit-client` (the package is `streamkit-client`, not `skit-cli`), then
  `target/release/skit-cli tune <session-id> web url <new-url>`. The embedded MCP endpoint is disabled by default, so
  REST/MCP tuning is not available out of the box.

## Cross-session leak checks
- Sequential: capture page A then page B; sampled B frames must contain no A pixels (use dark/light pages so pixel stats catch leaks).
- Concurrent: run a cast of A and a clip of B simultaneously (two curls with `&`); sample start/middle/end frames of both.
- Regression test: `cd plugins/native/servo && CARGO_TARGET_DIR=../../../target/plugins cargo test --release --test cross_session_leak` (heavy; llvmpipe; pass = exit 0).

## Devin Secrets Needed
None for local loopback testing (auth Auto disables auth).
