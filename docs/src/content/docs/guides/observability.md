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

### What's measured

Beyond HTTP and engine/node throughput, a few metric families are especially
useful for speech and ML workloads:

- **Plugin / ML inference** — native plugins emit per-call metrics labelled by
  `plugin_kind` (e.g. `whisper`, `kokoro`) and `op`: `plugin_call_duration_seconds`
  (histogram), `plugin_calls_total`, and `plugin_errors_total` /
  `plugin_timeouts_total` / `plugin_panics_total`. This is where inference
  latency and failures show up — usually the dominant cost of a speech pipeline.
- **Oneshot pipelines** — `oneshot_pipeline_duration` (histogram) is labelled by
  `status` (`ok`/`error`). Because every oneshot request hits the same
  `POST /api/v1/process` endpoint, splitting TTS vs STT relies on a bounded
  `service` label sourced from the pipeline's own `attributes` (see
  [Metric attributes](#metric-attributes) below); without it all oneshot traffic
  collapses into one series.
- **Speech gateway** — the [speech gateway example](https://github.com/streamer45/streamkit/tree/main/examples/speech-gateway)
  exposes Prometheus metrics for the front door it puts in front of skit:
  per-endpoint request rate/latency (`gateway_requests_total`,
  `gateway_request_duration_seconds`), in-flight gauge, upstream latency, and
  rejections by reason (`gateway_rejected_total`).

### Metric attributes

Pipelines can carry **bounded labels** on their metrics so you can break dashboards down by use case (e.g. `service=tts` vs `service=stt`) instead of collapsing every pipeline into one series.

A pipeline declares its own attributes in the definition:

```yaml
name: Speech-to-Text
mode: oneshot
attributes:
  service: stt
nodes: ...
```

`attributes` is a workload property — it describes *which* pipeline is running, not who called it — so the same value flows to both oneshot and dynamic runs.

The operator decides which attributes are allowed and how each is bounded, under `[server.metrics.attributes.<dimension>]` in `skit.toml`:

```toml
[server.metrics.attributes.service]
values   = ["tts", "stt"]   # enum allowlist; unknown/empty values clamp to `fallback`
fallback = "other"

# Omit `values` for a passthrough dimension — any non-empty declared value is
# emitted as-is (the operator opts into that cardinality), e.g. for `tenant`:
[server.metrics.attributes.tenant]
fallback = "unknown"
```

**Cardinality is operator-bounded.** A declared attribute whose key has **no** policy entry is dropped, never emitted — so a user-submitted oneshot pipeline can't inflate metric cardinality. With `values`, the declared value is trimmed + lowercased and matched against the allowlist; anything else (or an empty value) collapses to `fallback`.

**Declared-only contract.** If a pipeline omits a configured dimension, **no** label is emitted for it (rather than stamping `fallback` onto every pipeline). In PromQL the catch-all is still aggregated — `sum by (service)` groups the undeclared runs as `{service=""}` — so you keep uniform aggregation without forcing the label onto pipelines that never declared it.

**Coverage by mode:**

| Metric | Oneshot | Dynamic |
|--------|:-------:|:-------:|
| `oneshot_pipeline.duration` | ✓ | — (no pipeline-level duration metric) |
| `node.execution.duration` | ✓ | — (oneshot graph builder only) |
| `node.packets.*` | ✓ | ✓ |
| `node.state`, `engine.node.state_transitions`, `engine.nodes.active`, `pin_distributor.*` | — | ✓ (dynamic-engine instruments) |

`http.server.*` request metrics are **not** labeled — `service` is a pipeline property, so the breakdown lives on pipeline/node metrics, not on the HTTP layer.

### Run the full stack locally

To see all of the above on the dashboards without any cloud setup, use the
[`samples/observability`](https://github.com/streamer45/streamkit/tree/main/samples/observability)
compose stack — it wires skit (OTLP push) + the gateway (scrape) into Prometheus
and auto-provisions both dashboards in Grafana:

```bash
cd samples/observability
docker compose up -d
./generate-traffic.sh
# Grafana: http://localhost:3000
```

See its README for the wiring details and known gotchas (demo-image tag/plugin
layout, model-name matching, the Prometheus OTLP receiver, and local auth).

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
