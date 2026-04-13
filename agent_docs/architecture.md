<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# StreamKit Architecture

## Purpose

StreamKit is a self-hostable media processing server (written in Rust). A single
binary (`skit`) runs pipelines composed as a node graph (DAG) of built-in nodes,
plugins, and scriptable logic — via a web UI, YAML, or WebSocket API.

Two pipeline modes:

- **Dynamic (real-time):** Long-running, hot-reconfigurable sessions (voice
  agents, live streams) managed via the web UI or WebSocket API.
- **Oneshot (stateless):** Request/response batch processing (transcription,
  file conversion) via HTTP API.

## Workspace Structure

```
apps/skit/           Server binary — HTTP/WS handlers, config, auth, plugin management
apps/skit-cli/       CLI client binary (streamkit-client)
crates/core/         Shared traits and types — ProcessorNode, Pin, Packet, NodeRegistry
crates/engine/       Pipeline executor — graph_builder, oneshot engine, dynamic actor
crates/nodes/        All built-in processing nodes (audio, video, transport, core, containers)
crates/api/          YAML pipeline parsing, WebSocket protocol types, TS type generation
crates/plugin-native/ Host-side FFI adapter for native (.so) plugins
crates/plugin-wasm/  Host-side WASM plugin runtime (wasmtime)
sdks/plugin-sdk/     Plugin SDK for Rust, Go, and C (native + WASM targets)
ui/                  React web UI — node graph editor, compositor canvas, views
plugins/native/      Official ML plugins (Whisper, Kokoro, NLLB, SenseVoice, etc.)
samples/pipelines/   Example YAML pipeline definitions (dynamic/ and oneshot/)
e2e/                 Playwright end-to-end tests
docs/                Astro + Starlight documentation site (sidebar in docs/astro.config.mjs)
scripts/             Build, analysis, and marketplace tooling
```

## Crate Dependency Flow

```
server (apps/skit)  →  engine  →  nodes  →  core
       ↓                  ↓
      api ─────────────→ ↗        (api also depends on core)
       ↓
  plugin-native / plugin-wasm  →  core
```

(Arrows point from dependent to dependency.)

- **`streamkit-core`** defines the foundational abstractions: `ProcessorNode`
  trait, `NodeContext`, `Packet` (Audio/Video/Binary), `Pin` system
  (Input/Output with typed cardinality), `NodeRegistry`, `NodeState` lifecycle.
- **`streamkit-api`** defines the WebSocket message contract (JSON) and YAML
  pipeline format compilation.
- **`streamkit-engine`** takes a compiled pipeline graph and runs it.
  `graph_builder` wires nodes together. `oneshot` handles batch pipelines.
  The `dynamic` feature flag enables the `DynamicEngine` actor for live
  sessions with hot-reconfiguration.
- **`streamkit-nodes`** registers all built-in nodes organized by domain:
  `audio::` (codecs, filters), `video::` (colorbars, compositor, encoders),
  `transport::` (HTTP, MoQ, RTMP, MSE), `core::` (file I/O, script, pacer,
  passthrough), `containers::` (MP4, OGG, WAV, WebM).
- **`streamkit-server`** (`apps/skit`) is the final binary — axum HTTP server,
  WebSocket handlers, plugin loading, config, auth, MoQ gateway.

## Key Abstractions

| Concept | Description |
|---------|-------------|
| **Node** | Atomic processing unit implementing `ProcessorNode`, runs as a tokio task |
| **Pin** | Typed input/output port on a node (e.g., `audio/data`, `video/frame`) |
| **Packet** | Data unit flowing between pins — `AudioFrame`, `VideoFrame`, `Binary`, etc. |
| **PinCardinality** | Connection limits: `One`, `Broadcast`, `Dynamic` |
| **Passthrough** | Pin type that defers concrete type resolution to upstream |
| **NodeRegistry** | Factory + discovery system for all available node kinds |
| **DynamicEngine** | Actor managing a live node graph with add/remove/reconnect |
| **Oneshot** | Stateless batch pipeline — request in, result out, then teardown |

## Pipeline Lifecycle (Data Flow)

1. **Definition:** User writes YAML or uses the web UI to define a pipeline
2. **Compilation:** `streamkit_api::yaml` parses YAML → `Pipeline` struct with
   nodes, connections, and params
3. **Graph Building:** `engine::graph_builder` resolves pin types, validates
   connections, creates node instances from the registry
4. **Execution:** Each node runs as an async tokio task, connected via channels.
   The engine distributes packets between pins according to the graph topology.
5. **For dynamic pipelines:** The `DynamicEngine` actor accepts live mutations
   (add/remove nodes, update params, reconnect) via its `DynamicEngineHandle`.

## UI Architecture

The web UI (`ui/`) is a React 19 + TypeScript SPA:

- **State management:** Two complementary layers driven by WebSocket events:
  - **Jotai atoms** (`ui/src/stores/sessionAtoms.ts`) — primary store for
    high-frequency per-node data (states, stats, view data, params). Per-node
    atom families confine re-renders to the affected node's components.
  - **Zustand** (`ui/src/stores/sessionStore.ts`) — pipeline structure and
    connection management (low-frequency CRUD). Also receives node state/stats
    writes for transitional compatibility (being migrated to Jotai).
  - **React Query** (`@tanstack/react-query`) — REST API data (font/image/audio/
    plugin assets, session list).
- **WebSocket-driven:** The WS service (`ui/src/services/websocket.ts`) batches
  high-frequency updates via `requestAnimationFrame` and writes to Jotai atoms
  first, with a transitional Zustand write for consumers not yet migrated.
- **Views:** Design (node graph editor + compositor canvas), Monitor (live
  metrics), Convert (oneshot pipelines), Stream (dynamic MoQ pipelines).
- **Node graph:** Built on `@xyflow/react` (React Flow).
- **Compositor:** Custom canvas component with layer management, drag-resize,
  text/image overlays.
