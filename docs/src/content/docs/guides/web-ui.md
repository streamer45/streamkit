---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Using the Web UI
description: Guide to the StreamKit visual pipeline editor
---

StreamKit includes a React-based web interface for building and monitoring pipelines visually.

## Accessing the UI

Open [http://localhost:4545](http://localhost:4545) after starting the server:

```bash
just skit serve
# or
just dev  # For development with hot reload
```

## Dashboard Overview

The UI surfaces:

- **Sessions**: active pipelines and their status
- **Node palette**: built-in nodes + loaded plugins
- **Samples**: runnable example pipelines (from `samples/pipelines/`)
- **Plugins**: upload/unload plugins at runtime (if your role allows it)

## Creating a Session

1. Click **"New Session"** in the dashboard
2. Enter a name for your session
3. Choose a starting template or start blank

## Visual Pipeline Editor

### Adding Nodes

- Drag nodes from the **Node Palette** onto the canvas
- Or right-click the canvas and select from the context menu

### Connecting Nodes

- Click and drag from an **output pin** (right side) to an **input pin** (left side)
- Connections validate automatically based on data types

### Node Configuration

- Click a node to open its **Properties Panel**
- Modify parameters in real-time
- Changes apply immediately to the running pipeline

## Node States

Nodes display their current state with visual indicators:

| State | Description |
|-------|-------------|
| `Initializing` | Node is starting up |
| `Ready` | Node is ready; waiting for pipeline start |
| `Running` | Processing normally |
| `Recovering` | Attempting to recover from an error |
| `Degraded` | Running with reduced quality |
| `Failed` | Fatal error |
| `Stopped` | Node stopped (completed or shutdown) |

## Real-time Monitoring

The UI provides live updates via WebSocket:

- **Packet flow** visualization on connections
- **Node metrics** (packets processed, errors)
- **State changes** reflected instantly

## Telemetry Timeline

StreamKit also supports a per-session **telemetry bus** for timeline-style events (VAD start/end, transcription previews, LLM latency spans, etc.).

Telemetry is shown in the Monitor/Stream views when events are present. Events can come from:

- `core::script` (via the `telemetry.*` JavaScript API)
- Native plugins that emit out-of-band telemetry (e.g. `plugin::native::whisper` with `emit_vad_events: true`)
- Packet→telemetry adapters:
  - `core::telemetry_out` (terminal side-branch)
  - `core::telemetry_tap` (passthrough tap)

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

## Exporting Pipelines

1. Click **"Export"** in the toolbar
2. Save the YAML for sharing/versioning

To run exported YAML without the UI, send it to the server:

- Dynamic session: `POST /api/v1/sessions` with JSON `{ "name": "...", "yaml": "..." }`
- Oneshot: `POST /api/v1/process` with multipart field `config` (and optional `media`)

## Next Steps

- [Creating Pipelines](/guides/creating-pipelines/) - YAML syntax reference
- [Writing Plugins](/guides/writing-plugins/) - Extend with custom nodes
