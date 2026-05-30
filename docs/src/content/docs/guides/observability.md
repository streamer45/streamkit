---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Observability
description: Metrics, logs, and traces (OTLP) for running StreamKit in production
---

StreamKit supports **logs**, **metrics**, and **traces** via OpenTelemetry (OTLP), plus a sample Grafana dashboard.

## Logs

Configure console/file logging under `[log]` in `skit.toml`. See the [Configuration](/reference/configuration/) reference.

## Session Telemetry (UI Timeline)

Separate from OpenTelemetry, StreamKit has a per-session **telemetry bus** for high-level, timeline-style events (VAD start/end, transcription previews, LLM request/response latency, etc.). These are delivered over the WebSocket control plane as `nodetelemetry` events and are used by the web UI timeline.

Telemetry is **best-effort** and may be dropped under load. The server may truncate large string fields before forwarding to clients.

To produce telemetry:

- Add `core::telemetry_out` (side-branch) or `core::telemetry_tap` (passthrough) to convert packets like `Transcription` / `Custom` into timeline events.
- Use `core::script`’s `telemetry.emit/startSpan/endSpan` API for custom events and spans.
- Enable native plugin telemetry where available (e.g. `plugin::native::whisper`’s `emit_vad_events: true`).

Example: emit transcription telemetry without stalling the main pipeline:

```yaml
nodes:
  whisper_stt:
    kind: plugin::native::whisper
    params:
      emit_vad_events: true

  stt_telemetry:
    kind: core::telemetry_out
    params:
      packet_types: ["Transcription"]
      max_events_per_sec: 20
    needs:
      node: whisper_stt
      mode: best_effort
```

## Metrics (OTLP)

Metrics export is controlled by:

- `telemetry.enable`
- `telemetry.otlp_endpoint`
- `telemetry.otlp_headers` (optional)

### Prometheus (OTLP receiver)

Prometheus can ingest OTLP metrics when started with:

```bash
prometheus --web.enable-otlp-receiver
```

Point `telemetry.otlp_endpoint` at your Prometheus OTLP endpoint (see the Prometheus docs for the exact URL and supported protocols).

### Grafana dashboard

Import [`samples/grafana-dashboard.json`](https://github.com/streamer45/streamkit/blob/main/samples/grafana-dashboard.json) into Grafana and select the same Prometheus (or other OTLP-backed) datasource you're sending metrics to. This streamlined dashboard focuses on high-signal health indicators with collapsed advanced sections for debugging.

![Grafana Dashboard](/screenshots/grafana_dashboard.png)

### Metric naming (OTLP → Prometheus)

The OTLP→Prometheus exporter rewrites OTel names: dots become underscores, counters gain `_total`, histograms expand to `_bucket`/`_count`/`_sum`, and the unit is appended (`s` → `_seconds`, `By` → `_bytes`, `%` → `_percent`). So `plugin.call.duration` (unit `s`) is queried as `plugin_call_duration_seconds_bucket`, and the `plugin.kind` label becomes `plugin_kind`.

### Plugin / ML inference metrics

Native plugins (Whisper STT, Kokoro TTS, NLLB translation, …) are the hot path for hosted speech services, so their FFI calls are instrumented separately. All call metrics carry `plugin_kind` and `op` labels.

| Metric | Type | Description |
| --- | --- | --- |
| `plugin_call_duration_seconds` | histogram | FFI call latency. Use `histogram_quantile` over `_bucket` grouped by `le, plugin_kind` for p50/p95/p99. |
| `plugin_calls_total` | counter | FFI calls, by `plugin_kind`/`op`. |
| `plugin_errors_total` | counter | FFI call errors. |
| `plugin_timeouts_total` | counter | Caller-side timeouts (distinct from errors). |
| `plugin_panics_total` | counter | FFI calls that panicked. |
| `plugins_loaded` | gauge | Loaded plugins by `plugin_type` (`native`/`wasm`). |
| `plugin_operations_total` | counter | Load/unload operations, by `operation`/`plugin_type`. |

Overall failure rate per kind is `rate(plugin_errors_total) + rate(plugin_timeouts_total) + rate(plugin_panics_total)` — panics and timeouts are tracked apart from errors so they can be summed without double-counting. These power the dashboard's **Plugins / ML inference** row.

### Monitoring oneshot speech services

StreamKit's hosted speech endpoints (TTS/STT) run as oneshot pipelines, optionally behind the [`examples/speech-gateway`](https://github.com/streamer45/streamkit/tree/main/examples/speech-gateway) Go front-end.

- **Gateway:** the gateway exposes its own Prometheus `/metrics` endpoint (scrape it directly; it does not go through OTLP). It emits `gateway_requests_total{endpoint,method,code}`, `gateway_request_duration_seconds{endpoint}`, `gateway_inflight_requests{endpoint}`, `gateway_upstream_duration_seconds{endpoint}` (time spent in StreamKit vs. total), and `gateway_rejected_total{endpoint,reason}`. Comparing upstream vs. total latency isolates gateway overhead from inference time. These power the **Speech Gateway** row.
- **Per-service split:** `oneshot_pipeline_duration` carries a `service` label (`tts`/`stt`/`other`) so latency and error rate can be broken out per speech service without separate metrics. This powers the **Oneshot Speech Services** row.

## Traces (OTLP)

Tracing export is controlled by:

- `telemetry.tracing_enable`
- `telemetry.otlp_traces_endpoint` (required when tracing is enabled)

If you want a single place to receive both metrics and traces, run an OpenTelemetry Collector and forward data from there to Prometheus/Grafana Tempo/Jaeger.

## Tokio console (optional)

Enable `telemetry.tokio_console` to use `tokio-console` for async task diagnostics (requires a build with the `tokio-console` feature).

## Profiling (CPU, heap, and DHAT)

StreamKit also has optional profiling support intended for **local debugging** and trusted environments.

### CPU profiling (pprof)

When built with `--features profiling`, StreamKit exposes:

- `GET /api/v1/profile/cpu?duration_secs=30&format=flamegraph|protobuf&frequency=99`

For local dev, there are `just` helpers:

- Run server with profiling: `just skit-profiling serve`
- Fetch flamegraph: `just profile-flame 30 flamegraph.svg`
- Open pprof UI (requires Go): `just profile-web 30`

### Heap snapshots (jemalloc pprof)

When built with `--features profiling`, StreamKit exposes:

- `GET /api/v1/profile/heap`

Helpers:

- Fetch heap profile: `just heap-profile-fetch`
- Open pprof UI (requires Go): `just heap-profile-web`

### Allocation rate profiling (DHAT)

For allocation churn/hotspots, build with `--features dhat-heap` (mutually exclusive with `profiling`). DHAT writes `dhat-heap.json` on graceful shutdown.

Helpers:

- Run with DHAT: `just skit-dhat serve` (stop with Ctrl+C to generate `dhat-heap.json`)
- Open the viewer: `just dhat-view`

See [HTTP API](/reference/http-api/) for the full list of feature-gated endpoints.
