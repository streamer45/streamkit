<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# StreamKit Roadmap

This document outlines planned features and milestones for StreamKit. It's a living document that will evolve as priorities shift and community feedback comes in.

StreamKit is currently at **v0.1** (initial public release). This roadmap covers the path to **v1.0**.

---

## What I'm optimizing for (right now)

StreamKit is still a solo-driven project, so this roadmap is intentionally biased toward fundamentals:

- **Reliability**: predictable behavior, good failure modes, actionable errors
- **Ease of use**: "copy/paste → working", clear docs, sane defaults
- **Scalability**: repeatable load tests, measurable performance, operable deployments
- **Capabilities**: new nodes/plugins are welcome, but they'll be prioritized by real use cases from the community

## Near-Term (v0.1 → v0.5)

### Reliability & Developer Experience

- **Improved error messages** — Clearer diagnostics for pipeline validation and runtime errors (node/pin/type context, actionable hints)
- **API stabilization** — Stabilize HTTP/WebSocket APIs and schemas toward v1.0 with a clear deprecation story
- **Better defaults** — Safer config defaults (limits, timeouts, permissions) that work well in self-hosted environments
- **Docs + samples** — Expand "golden path" docs and sample pipelines so it's easy to try and easy to debug
- **End-to-end tests (Playwright)** — Add canonical UI/API e2e flows (create session, edit graph, inspect metrics, export) and run them in CI
- **Built-in authentication (optional)** — First-class authn/authz for the HTTP + WebSocket APIs (e.g., API keys and/or OIDC), with role assignment and audit-friendly logging

### Performance & Load Testing

- **Load test suite** — Curate a few canonical load scenarios (oneshot, dynamic, mixed) and track budgets over time (p95/p99 latency, throughput, CPU/mem)
- **Observability polish** — Make metrics/tracing consistent and production-friendly (dashboards that match docs, easier correlation from UI → logs)

### Capabilities (use-case driven)

- **VAD streaming mode** — Zero-latency audio passthrough with per-frame voice activity metadata, enabling downstream nodes to make real-time decisions without buffering delays
- **Multi-input HTTP oneshot** — Accept multiple input files in a single batch request (e.g., multiple audio tracks for mixing, or audio + subtitles for muxing)
- **S3 sink node** — Write pipeline output directly to S3-compatible storage
- **RTMP input node** — Ingest live streams from OBS, encoders, and other RTMP sources

### Transports & Connectivity

- **WebSocket transport nodes** — Subscriber/publisher/peer nodes for non-media packet streams and simpler "no-QUIC" deployments
- **Non-media MoQ examples** — Canonical examples that use MoQ/WebTransport for non-audio streams (events, data, RPC-like patterns) as a WS alternative

### Plugin Ecosystem (capability multiplier)

- Plugin contribution guidelines and examples
- Additional native plugin templates
- Documentation for building and distributing plugins

### Distribution & Platform Support

- **macOS binaries** — Ship native `skit` and `skit-cli` binaries for macOS (ARM64 for Apple Silicon, x86_64 for Intel). Core dependencies (opus, QuickJS) compile from source via Cargo and should work on macOS. Native plugins (Whisper, Kokoro, etc.) require additional work due to sherpa-onnx C library dependencies.
- **Multi-arch Docker images** — Build `linux/arm64` images alongside `linux/amd64` for better performance on Apple Silicon Macs running Docker.

### UI & Workflow

- **TypeScript support in script nodes** — Compile `.ts` scripts at load time for type-safe pipeline logic
- **UI code editor** — In-browser JavaScript/TypeScript editor with syntax highlighting and validation
- **Admin/Manage section** — Dedicated UI area for plugins, permissions/roles, secrets/config, and operational controls (separate from pipeline design/monitor views)

### Stability & Polish

- Expanded test coverage for nodes and plugins
- Documentation improvements and more sample pipelines

---

## Medium-Term (v0.5 → v1.0)

### Scalability & Ops

- **Multi-node clustering** — Separate control plane from media processing workers
- **Redis integration** — Shared state for distributed deployments
- **Kubernetes deployment** — Helm charts, HPA patterns, and scaling guides
- **Observability** — Enhanced metrics and tracing for production monitoring

### Multimodal Expansion (exploration)

StreamKit is media/processing-focused, not "audio-only". As real use cases emerge, expect exploration around:

- **Image packets** — Encoded and/or raw image frames as first-class packets
- **OCR nodes/plugins** — Text extraction pipelines (likely plugin-backed initially)
- **Event packets** — Structured events for routing/control (webhooks, metadata, detectors)

### Video Support

StreamKit is audio-first today. Video support is a major milestone for v1.0:

- **Video packet types** — Extend core to handle video frames alongside audio
- **Video codec plugins** — H.264, VP9, AV1 encoding/decoding
- **Compositing nodes** — Video mixing, overlays, and transformations
- **Container support** — MP4 and WebM muxing with video tracks

### Advanced Transports

- **WebRTC** — Bidirectional audio/video for browser-based applications
- **HLS/DASH output** — Adaptive bitrate streaming for live and VOD delivery
- **YouTube upload** — Direct publishing to YouTube from pipelines
- **RTMP/RTSP output** — Push streams to CDNs and media servers
- **SRT support** — Secure Reliable Transport for low-latency broadcast workflows

### Plugin System

- ResourceManager integration for native plugins (unified model caching)
- Plugin API versioning and compatibility checks
- Plugin-defined packet schemas/metadata ("virtual packet types") that surface in `/schema/packets` and the UI while flowing as `Custom(type_id)` at runtime
- Exploration of WASM/Native API convergence

---

## Beyond v1.0

Too early to tell.

---

## How to influence the roadmap

If you want something added (node, packet type, transport, plugin API), open an issue with:

- The concrete use case (what you're building)
- A minimal pipeline sketch (YAML or UI export)
- Expected performance/scale (throughput, latency, concurrency)
- "Built-in node" vs "plugin" preference (and why)

## Contributing

We welcome contributions toward any roadmap item. If you're interested in tackling something, please open an issue to discuss the approach first.

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.
