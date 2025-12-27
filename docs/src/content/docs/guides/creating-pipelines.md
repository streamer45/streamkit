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
      model_path: models/ggml-base.en-q5_1.bin
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
      input_broadcast: input
      output_broadcast: output
      allow_reconnect: true
    # Loopback: send processed output back into the peer's `in` pin.
    # Cycles are allowed for bidirectional nodes like `transport::moq::peer`.
    needs: opus_encoder

  opus_decoder:
    kind: audio::opus::decoder
    needs: moq_peer

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

- `needs` creates connections from each dependency's `out` pin to this node's input pin.
- If a node has a single dependency, it connects to `in`. If it has multiple dependencies, they connect to `in_0`, `in_1`, ... in the same order as the `needs` list.
- For pin cardinality (including dynamic pin families) and passthrough type inference rules, see [Pins & Type Inference](/reference/pins-and-types/).

## Connection Modes

Connections between nodes support two modes that control backpressure behavior:

| Mode | Description | Use Case |
|------|-------------|----------|
| `reliable` (default) | Synchronized backpressure - upstream waits for slow consumers | Main data flow, audio/video streams |
| `best_effort` | Drops packets when downstream buffer is full | Observers, metrics taps, debug outputs |

### When to Use Each Mode

**Reliable (default)**: Use for any connection where packet loss is unacceptable. The entire pipeline will slow down to match the slowest consumer. This is the right choice for audio/video streams where gaps would be noticeable.

**Best-effort**: Use for auxiliary outputs that shouldn't stall the main pipeline. Examples:

- Metrics collectors
- Debug loggers
- UI visualization taps
- Optional analytics

### Specifying Connection Mode

In the DAG format, use the object syntax for `needs` to specify a mode:

```yaml
nodes:
  moq_peer:
    kind: transport::moq::peer
    params:
      gateway_path: /moq
      input_broadcast: input
      output_broadcast: output
      allow_reconnect: true
    needs: opus_encoder

  gain:
    kind: audio::gain
    params:
      gain: 1.0
    needs: opus_decoder

  # Telemetry side-branch - best-effort (won't stall pipeline)
  telemetry:
    kind: core::telemetry_out
    params:
      packet_types: ["Binary"]
      max_events_per_sec: 5
    needs:
      node: gain
      mode: best_effort

  opus_decoder:
    kind: audio::opus::decoder
    needs: moq_peer

  opus_encoder:
    kind: audio::opus::encoder
    needs: gain
```

You can mix simple strings and objects in a list:

```yaml
  mixer:
    kind: audio::mixer
    needs:
      - input_a                    # reliable (default)
      - node: input_b
        mode: best_effort          # best-effort
```

The WebSocket API's `Connect` action also accepts the `mode` field:

```json
{
  "action": "connect",
  "session_id": "sess_xyz",
  "from_node": "gain1",
  "from_pin": "out",
  "to_node": "metrics",
  "to_pin": "in",
  "mode": "best_effort"
}
```

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
- `media` (optional)

Oneshot validation rules:

- If `media` is present: the pipeline must contain `streamkit::http_input`
- If `media` is absent: the pipeline must contain `core::file_reader` and must not contain `streamkit::http_input`
- Always: the pipeline must contain `streamkit::http_output`

```bash
curl -X POST http://localhost:4545/api/v1/process \
  -F config=@samples/pipelines/oneshot/speech_to_text.yml \
  -F media=@samples/audio/system/sample.ogg
```

## Updating Parameters at Runtime

Runtime tuning is done over the WebSocket control API at `GET /api/v1/control` (WebSocket upgrade). See the WebSocket API reference for message shapes.

## Next Steps

- [Web UI Guide](/guides/web-ui/) - Visual pipeline editing
- [Writing Plugins](/guides/writing-plugins/) - Create custom nodes
- [Node Reference](/reference/nodes/) - Complete node documentation
