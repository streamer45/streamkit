<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Web Capture Gateway

Render any web page to video through a StreamKit backend. The first path segment picks the output; paste the target URL after it — no flags, no JSON:

- **clip** — a finite MP4 file (oneshot pipeline): `web.streamkit.dev/clip/example.com`
- **cast** — a live stream, WebM or fMP4 (dynamic session): `web.streamkit.dev/cast/example.com`

The page is rendered by the [Servo](../../plugins/native/servo) browser engine on the backend. The gateway turns a pasteable URL into the multipart oneshot / dynamic-session calls the backend expects and, for `cast`, owns the session lifetime so abandoned streams are torn down instead of leaking a renderer.

## URL shape

```
{host}/{clip|cast}/[options/]{target-url}
```

The first segment is the output (`clip` or `cast`); the target URL follows **verbatim** (scheme optional, `https` assumed), so its own query string just rides along — no percent-encoding:

```
web.streamkit.dev/clip/grafana.example/d/abc?panel=3&from=now-6h
```

Options are an optional comma-separated `key=value` segment between the output and the URL:

```
web.streamkit.dev/clip/dur=30s/example.com                # clip length (default 10s, capped at 60s)
web.streamkit.dev/cast/res=2560x1440/example.com          # capture resolution (default 1920x1080)
web.streamkit.dev/clip/res=1280x720,dur=15s/example.com   # combine, comma-separated
```

`dur` is `clip`-only; `res` (the capture resolution) applies to both modes. The scheme is identical locally, e.g. `http://127.0.0.1:8080/clip/dur=5s/example.com`.

The same URL serves a browser and a player/script: an address-bar visit (`Accept: text/html`) to a `cast` URL returns a tiny autoplay page; anything else (a `<video src>`, `curl`, `ffplay`) gets the raw stream/file.

## Formats & browser support

| Mode | Format | Plays in |
|------|--------|----------|
| `clip` | MP4 / H.264, fragmented (plays inline as it renders) | Everywhere, including Safari/iOS |
| `cast` | VP9 / WebM by default; H.264 / fMP4 via `--cast-encoder h264-sw` | VP9: Chromium & Firefox. H.264/fMP4: everywhere, including Safari/iOS |

The cast default (VP9/WebM) is crisper for screen text but plays only in Chromium/Firefox. For universal live playback (incl. Safari/iOS), start the gateway with `--cast-encoder h264-sw` (or `h264-hw`): `transport::http::mse` serves fragmented MP4, which a plain `<video>` plays everywhere. The encoder is an instance-wide setting, not a per-request option.

## Prereqs

- A StreamKit server with the **Servo plugin** built and loaded:
  `just build-plugin-native-servo && just copy-plugins-native`.
- A non-admin **`user`** token suffices (it may create/destroy sessions and use `transport::http::mse`, `plugin::native::servo`, and the video/container nodes) — no admin needed. Pass it via `--token` / `SKIT_TOKEN`.
- Go 1.24+.

## Run

```sh
cd examples/web-capture
go run ./cmd/gateway --listen :8080 --skit-url http://127.0.0.1:4545

# clip → MP4 (plays inline in a browser; -o saves it)
curl -L 'http://127.0.0.1:8080/clip/dur=5s/example.com' -o clip.mp4

# cast → open in a browser to watch live
xdg-open 'http://127.0.0.1:8080/cast/example.com'
```

### Configuration

| Env | Flag | Default | Purpose |
|-----|------|---------|---------|
| `GATEWAY_LISTEN` | `--listen` | `:8080` | Listen address |
| `SKIT_URL` | `--skit-url` | `http://127.0.0.1:4545` | Backend URL |
| `SKIT_TOKEN` | `--token` | — | Bearer token sent to the backend |
| `GATEWAY_MAX_CONCURRENCY` | `--max-concurrency` | 4 | Max concurrent clip renders |
| `GATEWAY_CLIP_DEFAULT_SECS` | `--clip-default-secs` | 10 | Default clip duration |
| `GATEWAY_CLIP_MAX_SECS` | `--clip-max-secs` | 60 | Maximum clip duration |
| `GATEWAY_MAX_SESSIONS` | `--max-sessions` | 8 | Max concurrent live cast sessions |
| `GATEWAY_MAX_VIEWERS` | `--max-viewers` | 10 | Max viewers per cast stream |
| `GATEWAY_SESSION_IDLE_SECS` | `--session-idle-secs` | 30 | Idle grace before a viewerless session is reaped |
| `GATEWAY_SESSION_MAX_SECS` | `--session-max-secs` | 1800 | Hard cap on a session's lifetime |
| `GATEWAY_RESOLUTION` | `--resolution` | `1920x1080` | Capture resolution WxH (render = encode, 1:1) |
| `GATEWAY_CLIP_BITRATE_KBPS` | `--clip-bitrate-kbps` | `10000` | Clip video bitrate (kbps) |
| `GATEWAY_CAST_BITRATE_KBPS` | `--cast-bitrate-kbps` | `6000` | Cast video bitrate (kbps) |
| `GATEWAY_CLIP_ENCODER` | `--clip-encoder` | `h264-sw` | `h264-sw`, or `h264-hw` (Vulkan Video) |
| `GATEWAY_CAST_ENCODER` | `--cast-encoder` | `vp9-sw` | `vp9-sw`, `av1-sw`, `av1-hw`, `h264-sw`, `h264-hw` |
| `GATEWAY_LOAD_TIMEOUT_SECS` | `--load-timeout-secs` | 5 | Max first-frame hold while the page loads (servo `load_timeout_secs`) |

Capture runs at **1080p30** by default: the page renders **and** encodes at the same resolution (1:1, no downscale), so text stays crisp and the page gets a real desktop layout. Drop to `res=1280x720` for less bandwidth or raise to `res=2560x1440` for more detail (capped at 4K); frame rate is fixed at 30.

### Encoders

Software encoders are the default, so the gateway runs anywhere with no GPU — ideal for local dev. Hardware is opt-in per mode:

| Profile | Codec / container | Modes | Notes |
|---|---|---|---|
| `h264-sw` | H.264 — MP4 (clip) / fMP4 (cast) | clip, cast | universal incl. Safari/iOS; OpenH264 (software) |
| `h264-hw` | H.264 — MP4 / fMP4 | clip, cast | Vulkan Video (NVIDIA/AMD/Intel; NVENC has no H.264) |
| `vp9-sw` | VP9 — WebM | cast | cast default; crisp screen text; Chromium/Firefox |
| `av1-sw` | AV1 — WebM | cast | SVT-AV1 (software) — exercise AV1 without a GPU |
| `av1-hw` | AV1 — WebM | cast | NVENC AV1 (NVIDIA); efficient + crisp; Chromium/Firefox |

Clips are always H.264/MP4 (universal download). Cast defaults to VP9/WebM (crisp, Chromium/Firefox); switch to `h264-sw`/`h264-hw` for H.264/fMP4 that also plays in Safari/iOS, or `av1-*` for AV1/WebM. Hardware profiles require the matching runtime in the **backend** (Vulkan for `h264-hw`, CUDA + NVENC for `av1-hw`) — the gateway only selects the node, so a missing runtime surfaces as a backend pipeline error, not a gateway one.

## Lifetime & teardown (cast)

The engine does **not** stop a pipeline when its MSE viewers disconnect, and has no idle reaper — so lifetime is the gateway's job. It keeps **one session per normalized URL** (viewers of the same page share one renderer), refcounts viewers across the streaming proxy, and a background reaper tears down sessions that have been **idle** past the grace window or have exceeded their **max lifetime** (via `DELETE /api/v1/sessions/{id}`, which fully drops the pipeline). On `SIGTERM` it drains and destroys every owned session, so a redeploy never leaks renderers.

## Metrics

Prometheus metrics at `GET /metrics` (not gated by the concurrency limit). Beyond the per-endpoint request/latency/rejection series (`endpoint` is `clip` or `cast`), `cast` adds `webcapture_active_sessions`, `webcapture_active_viewers`, `webcapture_session_lifetime_seconds`, and `webcapture_sessions_reaped_total{reason}`. A public instance may keep `/metrics` inside its trust boundary rather than exposing it.

## Limitations

- **Public / URL-only.** Targets that resolve to loopback/private/link-local/CGNAT/cloud-metadata addresses are rejected (SSRF guard). Auth-gated pages (cookies/headers) are intentionally out of scope — a future self-hosted feature, never the public path. DNS rebinding between the gateway's check and Servo's own fetch is a known gap.
- **Servo is view-only and software-rendered**: static/CSS pages render at full frame rate; heavy WebGL is ~15–20 fps. No clicking, scrolling, or login.
- **`cast` in Safari/iOS**: the default VP9/WebM is Chromium/Firefox-only; run with `--cast-encoder h264-sw` for fMP4/H.264 that plays in Safari/iOS.
