<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Speech Gateway

Thin HTTP gateway that rewrites simple STT/TTS requests into the multipart oneshot format expected by a StreamKit backend.

## Prereqs

- StreamKit server running locally (default assumed: `http://127.0.0.1:4545`).
- Go 1.24+.

## Run the gateway

```sh
cd examples/streamkit-cli-gateway
go run ./cmd/gateway --listen :8080 --skit-url http://127.0.0.1:4545
```

Environment equivalents:

- `GATEWAY_LISTEN` (default `:8080`)
- `SKIT_URL` (default `http://127.0.0.1:4545`)
- `SKIT_TOKEN` (optional bearer sent to Skit)
- `GATEWAY_MAX_CONCURRENCY` (default 10)
- `GATEWAY_MAX_BODY_BYTES` (default 1MB)
- `GATEWAY_MAX_TTS_TEXT_SIZE` (default 1000 characters)

## STT via curl (Ogg/Opus)

Transcribe a file:

```sh
curl -H "Content-Type: audio/ogg" --data-binary @speech.ogg http://127.0.0.1:8080/stt
```

Transcribe from microphone (requires ffmpeg):

```sh
./stt.sh
```

Press Ctrl-C when done speaking. The script captures audio, sends it to the gateway, and displays the transcription.

Response is NDJSON (one JSON object per line).

## TTS via curl (plain text)

```sh
curl -H "Content-Type: text/plain" --data 'Hello from StreamKit' http://127.0.0.1:8080/tts | ffplay -nodisp -autoexit -
```

Response is `audio/ogg` (Opus mono).

## Metrics

The gateway exposes Prometheus metrics at `GET /metrics` (via `promhttp`). This
route is **not** gated by `GATEWAY_MAX_CONCURRENCY`, so it stays scrapable even
when all request slots are in use. A public/hosted instance (e.g. behind
`tts.streamkit.dev` / `stt.streamkit.dev`) may choose not to expose `/metrics`
externally — scrape it from inside the trust boundary instead.

Every metric carries an `endpoint` label whose value is exactly `tts` or `stt`. The `method` label on `gateway_requests_total` is folded to `other` for any method outside `{GET,HEAD,POST,PUT}` to bound label cardinality.

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `gateway_requests_total` | counter | `endpoint`, `method`, `code` | Requests served, by HTTP method and status code. |
| `gateway_request_duration_seconds` | histogram | `endpoint` | Total handler latency. |
| `gateway_inflight_requests` | gauge | `endpoint` | In-flight requests (received, not yet completed); includes time queued on the concurrency semaphore, so it can exceed `GATEWAY_MAX_CONCURRENCY`. |
| `gateway_upstream_duration_seconds` | histogram | `endpoint` | Time to receive response headers from the skit backend `/api/v1/process` (excludes streaming the body to the client). |
| `gateway_rejected_total` | counter | `endpoint`, `reason` | Gateway-side rejections, recorded at the rejection site (not inferred from forwarded status). `reason` ∈ `bad_content_type`, `too_large`, `upstream_error`. |

Histogram buckets are tuned for multi-second STT/TTS workloads:
`0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10, 30` seconds.

```sh
curl http://127.0.0.1:8080/metrics
```
