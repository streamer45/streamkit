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

Next:
- [Creating Pipelines](/guides/creating-pipelines/)
- [HTTP API](/reference/http-api/)
- [Writing Plugins](/guides/writing-plugins/)
