<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Speech Gateway

Thin HTTP gateway that rewrites simple STT/TTS requests into the multipart oneshot format expected by a StreamKit backend.

## Hosted instance

A free, best-effort public instance runs at `https://tts.streamkit.dev` and `https://stt.streamkit.dev`, so you can try the endpoints below without running anything:

Text to speech (returns Opus audio, piped to `ffplay`):

```sh
curl -d 'Hello from StreamKit' https://tts.streamkit.dev | ffplay -nodisp -autoexit -
```

Speech to text — record from your mic with `ffmpeg`, no audio file needed (use `STT_URL=https://stt.streamkit.dev ./stt.sh` for an interactive, cross-platform version):

```sh
# macOS; on Linux use "-f pulse -i default" (PulseAudio/PipeWire) or "-f alsa -i default"
ffmpeg -hide_banner -f avfoundation -i ":0" -t 5 -ac 1 -ar 48000 -c:a libopus -f ogg - | curl -s --data-binary @- -H 'Content-Type: audio/ogg' https://stt.streamkit.dev | jq
```

There is no SLA — it may be slow, rate-limited, or offline at any time, and usage is monitored for abuse. Don't send anything sensitive. Run your own (below) to remove those limits.

## Prereqs

- StreamKit server running locally (default assumed: `http://127.0.0.1:4545`).
- Go 1.24+.

## Run the gateway

```sh
cd examples/speech-gateway
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

Response is NDJSON (one JSON object per line). The gateway flattens the backend's
tagged `Packet` envelope, so each line is the bare transcription object:

```json
{"text": "…", "segments": [ … ], "language": "en", "metadata": null}
```

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

Every metric carries an `endpoint` label whose value is exactly `tts` or `stt`. To bound label cardinality on `gateway_requests_total`, the `method` label folds to `other` outside `{GET,HEAD,POST,PUT}`, and the `code` label keeps canonical HTTP statuses (and `499` for client-closed) but folds non-canonical backend codes to their class (e.g. `5xx`, or `other` outside `[100,599)`).

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `gateway_requests_total` | counter | `endpoint`, `method`, `code` | Requests served, by HTTP method and status-code class (see cardinality note above). |
| `gateway_request_duration_seconds` | histogram | `endpoint` | Total handler latency. |
| `gateway_inflight_requests` | gauge | `endpoint` | In-flight requests (received, not yet completed); includes time queued on the concurrency semaphore, so it can exceed `GATEWAY_MAX_CONCURRENCY`. |
| `gateway_upstream_duration_seconds` | histogram | `endpoint` | Time to receive response headers from the skit backend `/api/v1/process` (excludes streaming the body to the client). |
| `gateway_rejected_total` | counter | `endpoint`, `reason` | Gateway-side rejections, recorded at the rejection site (not inferred from forwarded status). `reason` ∈ `bad_content_type`, `too_large`, `upstream_error`. |

Histogram buckets are tuned for multi-second STT/TTS workloads:
`0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10, 30` seconds.

```sh
curl http://127.0.0.1:8080/metrics
```

### Grafana dashboard

A ready-made dashboard lives at [`grafana-dashboard.json`](./grafana-dashboard.json). It is self-contained but draws from **two metric sources**, so point Grafana at a Prometheus that sees both:

- **Gateway metrics** (`gateway_*`): Prometheus **scrapes** the gateway's `/metrics` endpoint directly.
- **Backend metrics** (`oneshot_pipeline_duration`, `plugin_call_duration_seconds`, `plugin_calls_total`, …): skit does **not** expose a `/metrics` scrape endpoint — it **pushes** via OTLP. Run an OTLP collector (e.g. the OpenTelemetry Collector with a `prometheus` exporter, or a Prometheus OTLP receiver) and have Grafana's Prometheus read from there.

The dashboard's **Oneshot Speech Services** row splits `oneshot_pipeline_duration` by a `service` label (`tts`/`stt`). That label is sourced from each pipeline's `attributes: {service: ...}` block — the gateway's embedded STT/TTS pipelines declare it — and is only emitted when the operator allowlists the dimension in `skit.toml` (see the config below). Without that allowlist the backend rows stay empty (the `service` label is dropped). See the [observability guide](https://github.com/streamer45/streamkit/blob/main/docs/src/content/docs/guides/observability.md) for details.

### Wiring up Prometheus end-to-end

The two sources reach one Prometheus by two different paths: Prometheus
**scrapes** the gateway, and skit **pushes** to Prometheus's OTLP receiver.

1. Run Prometheus with its OTLP receiver enabled (Prometheus ≥ 2.47):

   ```sh
   prometheus --config.file=prometheus.yml --web.enable-otlp-receiver
   ```

   ```yaml
   # prometheus.yml — scrape the gateway directly
   scrape_configs:
     - job_name: speech-gateway
       static_configs:
         - targets: ["127.0.0.1:8080"]   # gateway /metrics
   ```

2. Point skit's OTLP exporter at that receiver (in `skit.toml`) and allowlist
   the `service` dimension so the per-service rows populate:

   ```toml
   [telemetry]
   enable       = true
   otlp_endpoint = "http://127.0.0.1:9090/api/v1/otlp/v1/metrics"

   [server.metrics.attributes.service]
   values   = ["tts", "stt"]
   fallback = "other"
   ```

Prefer an [OpenTelemetry Collector](https://opentelemetry.io/docs/collector/)
(OTLP receiver → `prometheus` exporter) in front of skit if you need
buffering, fan-out, or relabeling; point `otlp_endpoint` at the collector and
have Prometheus scrape the collector instead. Either way, set Grafana's
Prometheus datasource to this instance when importing the dashboard.

To run the gateway, Prometheus, and Grafana together locally with everything
pre-wired, see [`samples/observability`](../../samples/observability).
