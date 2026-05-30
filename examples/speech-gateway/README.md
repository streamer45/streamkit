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

## Observability

The gateway exposes a Prometheus `/metrics` endpoint (scrape it directly — it does not go through StreamKit's OTLP pipeline):

| Metric | Type | Labels | Notes |
| --- | --- | --- | --- |
| `gateway_requests_total` | counter | `endpoint`, `method`, `code` | Request rate and status mix. |
| `gateway_request_duration_seconds` | histogram | `endpoint` | End-to-end gateway latency. |
| `gateway_upstream_duration_seconds` | histogram | `endpoint` | Time spent in StreamKit; gap vs. total is gateway overhead. |
| `gateway_inflight_requests` | gauge | `endpoint` | Concurrent in-flight requests. |
| `gateway_rejected_total` | counter | `endpoint`, `reason` | Rejections from `GATEWAY_MAX_CONCURRENCY` / `GATEWAY_MAX_BODY_BYTES` / `GATEWAY_MAX_TTS_TEXT_SIZE`. |

A ready-made Grafana dashboard lives at [`grafana-dashboard.json`](./grafana-dashboard.json). It is self-contained: import it and pick the Prometheus datasource scraping both the gateway and the StreamKit backend. It includes the gateway metrics above, a per-service split of the backend's `oneshot_pipeline_duration` (via the `service` label: `tts`/`stt`/`other`), and the StreamKit native-plugin inference metrics (`plugin_call_duration_seconds`, `plugin_calls_total`, …) that back the STT/TTS models.
