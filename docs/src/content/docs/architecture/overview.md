---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Architecture Overview
description: How StreamKit is structured at a high level
---

StreamKit has three major pieces:

- **Server (`skit`)**: the Rust backend that runs pipelines and serves the web UI + APIs.
- **Pipelines engine**: compiles YAML into a typed node graph (DAG) and executes it as Tokio tasks connected by bounded channels.
- **Web UI**: a React app for creating, running, and monitoring pipelines in real time, with a dedicated compositor scene editor for video layouts.

## Execution surfaces

- **HTTP API** for oneshot request/response pipelines (`POST /api/v1/process`).
- **WebSocket control plane** for long-running dynamic sessions (`GET /api/v1/control`).
- **MoQ/WebTransport (QUIC/UDP)** for real-time media transport (when enabled), using the same port as the HTTP server.

## Extensibility

- **Built-in nodes** (core, audio, video, containers, transport) — including a multi-layer video compositor with CPU (tiny-skia) and GPU (wgpu) backends.
- **Plugins**: native (in-process C ABI) and WASM (sandboxed Component Model) — e.g. Slint for dynamic UI overlays, Whisper/SenseVoice for STT, Kokoro/Piper for TTS.
- **Script node**: sandboxed JavaScript (QuickJS) for lightweight integration and text processing.

## Crate layout

| Crate | Path | Purpose |
|-------|------|---------|
| `streamkit-server` | `apps/skit/` | Server binary — HTTP/WS handlers, config, auth, plugins, MCP |
| `streamkit-client` | `apps/skit-cli/` | CLI client binary (`skit-cli`) |
| `streamkit-core` | `crates/core/` | Shared traits/types — `ProcessorNode`, `Pin`, `Packet`, `NodeRegistry` |
| `streamkit-engine` | `crates/engine/` | Pipeline executor — graph builder, oneshot engine, dynamic actor |
| `streamkit-nodes` | `crates/nodes/` | Built-in nodes (`audio::`, `video::`, `transport::`, `core::`, `containers::`) |
| `streamkit-api` | `crates/api/` | YAML pipeline parsing, WebSocket protocol, TS type generation |
| `streamkit-plugin-native` | `crates/plugin-native/` | Native plugin host adapter (C ABI / FFI) |
| `streamkit-plugin-wasm` | `crates/plugin-wasm/` | WASM plugin host adapter (Component Model) |

The Web UI (`ui/`) is a React 19 + TypeScript app built with Vite and Bun. Official plugins live under `plugins/native/`.

## Data flow

1. A pipeline YAML is parsed by the `api` crate into a validated graph.
2. The `engine` crate compiles the graph into concrete `ProcessorNode` instances (from `core`) and connects them via bounded async channels.
3. Each node runs as an independent async task. Packets flow through connections, with backpressure controlled by channel bounds.
4. For dynamic sessions, the engine actor handles runtime graph mutations (add/remove nodes, connect/disconnect) via the WebSocket control API.

Next:
- [Creating Pipelines](/guides/creating-pipelines/)
- [HTTP API](/reference/http-api/)
- [Writing Plugins](/guides/writing-plugins/)
