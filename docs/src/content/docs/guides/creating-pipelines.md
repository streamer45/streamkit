---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Creating Pipelines
description: Learn how to define and configure processing pipelines
---

Pipelines in StreamKit define a processing graph of nodes. The server accepts pipelines as YAML and compiles them into an internal DAG representation.

## Pipeline Formats

StreamKit supports two YAML formats for defining pipelines:

### Linear Format (steps)

For simple sequential pipelines:

```yaml
name: transcribe-audio
mode: oneshot
steps:
  - kind: streamkit::http_input
  - kind: containers::ogg::demuxer
  - kind: audio::opus::decoder
  - kind: audio::resampler
    params:
      target_sample_rate: 16000
  - kind: plugin::native::whisper
    params:
      model_path: models/ggml-tiny.en-q5_1.bin
  - kind: core::json_serialize
  - kind: streamkit::http_output
```

### DAG / `needs` Format

For pipelines with branching or explicit node IDs, use a map keyed by node ID. Dependencies are expressed via `needs`:

```yaml
name: realtime-echo
mode: dynamic
nodes:
  # Bidirectional MoQ endpoint backed by the server's built-in MoQ gateway.
  # Clients publish to `{gateway_path}/input` and subscribe to `{gateway_path}/output`.
  moq_peer:
    kind: transport::moq::peer
    params:
      gateway_path: /moq
      input_broadcasts: [input]
      output_broadcast: output
      allow_reconnect: true
    # Loopback: send processed output back into the peer's `in` pin.
    # Cycles are allowed for bidirectional nodes like `transport::moq::peer`.
    needs: opus_encoder

  opus_decoder:
    kind: audio::opus::decoder
    needs:
      in: moq_peer.audio/data

  gain:
    kind: audio::gain
    params:
      gain: 1.0
    needs: opus_decoder

  opus_encoder:
    kind: audio::opus::encoder
    needs: gain
```

Notes:

- `transport::moq::peer` uses `input_broadcasts` (a list). The first entry is the primary publisher broadcast.
- `needs` creates connections from each dependency's `out` pin to this node's input pin.
- If a node has a single dependency, it connects to `in`. If it has multiple dependencies, they connect to `in_0`, `in_1`, ... in the same order as the `needs` list.
- For pin cardinality (including dynamic pin families) and passthrough type inference rules, see [Pins & Type Inference](/reference/pins-and-types/).

## Connection Modes

Connections between nodes support two modes that control backpressure behavior:

| Mode | Description | Use Case |
|------|-------------|----------|
| `reliable` (default) | Synchronized backpressure — upstream waits for slow consumers | Main data flow, audio/video streams |
| `best_effort` | Drops packets when downstream buffer is full | Observers, metrics taps, debug outputs |

In the DAG format, use the object syntax for `needs` to specify a mode:

```yaml
  mixer:
    kind: audio::mixer
    needs:
      - input_a                    # reliable (default)
      - node: input_b
        mode: best_effort          # best-effort
```

For a worked example using `best_effort` to branch telemetry off a main pipeline, see the [Observability guide](/guides/observability/#session-telemetry-ui-timeline).

The WebSocket API's `Connect` action also accepts a `mode` field. See the [WebSocket API reference](/reference/websocket-api/) for details.

## Fanout, Backpressure, and Buffers

Most nodes expose a `broadcast` output pin (typically `out`), meaning a single output can feed multiple downstream nodes. Internally, the engine uses **bounded async channels** between nodes and maintains per-connection buffering so one slow consumer doesn't require unbounded memory.

How this behaves depends on the connection mode:

- **`reliable`**: a slow downstream consumer backpressures the upstream sender; with fanout, the effective throughput can be limited by the slowest consumer.
- **`best_effort`**: if a downstream buffer is full, packets for that specific connection are dropped and the upstream sender continues (useful for observers and taps).

The main tuning knobs for these queues live under `[engine]` in `skit.toml` (e.g. `node_input_capacity`, `pin_distributor_capacity`, and oneshot `media_channel_capacity`). See:

- [Performance Tuning](/guides/performance/)
- [Configuration](/reference/configuration/)

## Pipeline Modes

| Mode | Description | Typical Use |
|------|-------------|-------------|
| `oneshot` | Runs to completion and returns an HTTP response | File conversion, TTS, STT |
| `dynamic` | Long-running session managed by the server | Live pipelines via Web UI |

## Running Pipelines

### Dynamic sessions

The easiest way to create/manage dynamic sessions is with `skit-cli` (it wraps the HTTP API):

```bash
skit-cli create my-pipeline.yml --name my-session
skit-cli list
skit-cli destroy my-session
```

If you want to call the API directly, create a session by sending pipeline YAML to `POST /api/v1/sessions`:

```bash
curl -X POST http://localhost:4545/api/v1/sessions \
  -H "Content-Type: application/json" \
  -d '{
    "name": "my-session",
    "yaml": "mode: dynamic\nsteps:\n  - kind: core::json_serialize\n"
  }'
```

Then:

```bash
curl http://localhost:4545/api/v1/sessions
curl http://localhost:4545/api/v1/sessions/<id-or-name>/pipeline
```

### Oneshot processing (HTTP multipart)

Use `POST /api/v1/process` with multipart fields:

- `config` (YAML, required; must be the first field)
- Upload fields for media (optional): names must match `streamkit::http_input` nodes. Default is `media` when a single `http_input` exists with no params; otherwise use the node id or `params.field`. If `params.fields` is set, only the listed fields are accepted and the legacy `media` field is disabled.

Oneshot validation rules:

- If uploads are present: the pipeline must contain `streamkit::http_input` (field names must match)
- If uploads are absent: the pipeline must contain `core::file_reader` and must not contain `streamkit::http_input`
- Always: the pipeline must contain `streamkit::http_output`

```bash
# Add -H "Authorization: Bearer $TOKEN" if built-in auth is enabled.
curl -X POST http://localhost:4545/api/v1/process \
  -F config=@samples/pipelines/oneshot/double_volume.yml \
  -F media=@samples/audio/system/sample.ogg \
  --output out.ogg
```

## Updating Parameters at Runtime

Runtime tuning is done over the WebSocket control API at `GET /api/v1/control` (WebSocket upgrade). See the WebSocket API reference for message shapes.

## Next Steps

- [Web UI Guide](/guides/web-ui/) - Visual pipeline editing
- [Writing Plugins](/guides/writing-plugins/) - Create custom nodes
- [Node Reference](/reference/nodes/) - Complete node documentation
