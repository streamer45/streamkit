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

Import `samples/grafana-dashboard.json` into Grafana and select the same Prometheus (or other OTLP-backed) datasource you’re sending metrics to.

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
