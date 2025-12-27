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

## Main Routes

The Web UI has four main routes:

- **Design** (default): build and edit pipelines visually.
- **Monitor**: inspect and manage live sessions.
- **Convert**: demo/test **oneshot** pipelines (request → response).
- **Stream**: demo/test **dynamic** pipelines (MOQ-powered streaming sessions).

## Design View

Design View is the default route and is split into three panes:

- **Left pane**: library and tools:
  - **Nodes**: the node palette/library (built-ins + loaded plugins).
  - **Plugins**: view/manage loaded plugins (availability depends on your role/config).
  - **Samples**: example pipelines you can load as a starting point.
  - **Fragments**: reusable building blocks you can drop into a graph.
- **Center pane (canvas)**: a React Flow editor where you:
  - Drag and drop nodes onto the canvas.
  - Connect nodes by drawing edges between ports.
  - Use the right-click context menu for actions like import/export.
- **Right pane**:
  - **YAML**: the pipeline definition for the canvas (two-way synced).
  - **Inspector** (when a node is selected): inspect/tune that node’s parameters.

### What is a Fragment?

A fragment is a reusable mini-graph (a small, pre-wired set of nodes) that you can insert into a larger pipeline. Use fragments to share common patterns (e.g. “input → preprocess → STT”) without rebuilding them by hand.

## Monitor View

Monitor View uses the same overall three-pane layout, but focuses on running sessions:

- **Left pane**: a live list of sessions until you enter **Staging Mode** (then it switches to the node library/palette for editing).
- **Center pane**: the session graph view.
- **Right pane** (once a session is selected): the YAML editor plus the Inspector pane for selected nodes.

## Convert View

Convert is for demoing/testing **oneshot** pipelines: load or author a pipeline, run it against input media, and review outputs.

## Stream View

Stream is for demoing/testing **dynamic** (long-running) pipelines using MOQ-powered streaming sessions.

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

In Design View, use the canvas right-click context menu to export/import pipelines. The YAML pane is also a convenient way to copy/paste pipelines for sharing/versioning.

To run exported YAML without the UI, send it to the server:

- Dynamic session: `POST /api/v1/sessions` with JSON `{ "name": "...", "yaml": "..." }`
- Oneshot: `POST /api/v1/process` with multipart field `config` (and optional `media`)

## Next Steps

- [Creating Pipelines](/guides/creating-pipelines/) - YAML syntax reference
- [Writing Plugins](/guides/writing-plugins/) - Extend with custom nodes
