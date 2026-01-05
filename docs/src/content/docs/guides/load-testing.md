---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Load Testing
description: Run targeted stress tests and capture profiles for StreamKit
---

StreamKit ships with a small load-test runner (`skit-cli loadtest`) plus a set of ready-made configs under `samples/loadtest/`.

Use these when you want to:

- Reproduce and profile a specific hotspot (codec, muxing, mixing, control plane, session lifecycle).
- Compare changes across runs with consistent inputs.
- Stress a single subsystem without the UI in the loop.

## Prerequisites

- Start the server: `just skit serve`
- Load tests use the client binary: `just skit-cli -- lt <config>`
- Some presets require a local MoQ relay at `http://localhost:4443`

## Running Presets

All configs live in `samples/loadtest/`. You can run them via a single `just` target:

- `just lt <id>` runs `samples/loadtest/<id>.toml`
- `just lt <path>` runs an explicit TOML path

Examples:

- `just lt stress-oneshot`
- `just lt oneshot-opus-transcode-fast`
- `just lt stress-dynamic sessions=10` (or `just lt stress-dynamic --sessions 10`)
- `just lt dynamic-tune-heavy --cleanup`

### Oneshot (HTTP batch pipelines)

- `just lt stress-oneshot` — default oneshot stress preset
- `just lt oneshot-http-passthrough` — multipart upload + oneshot engine overhead (minimal node CPU)
- `just lt oneshot-graph-chain` — graph wiring + channel hops (passthrough chain)
- `just lt oneshot-opus-transcode-fast` — codec-heavy (Ogg demux + Opus decode/encode), no pacer

### Dynamic (long-lived sessions)

- `just lt stress-dynamic` — default dynamic stress preset
- `just lt dynamic-scale-audio-gain` — many sessions, sustained decode, low tune rate
- `just lt dynamic-tune-heavy` — stresses control plane param updates (frequent tuning, many `audio::gain` nodes)
- `just lt dynamic-moq-fanout` — MoQ fanout (requires relay at `http://localhost:4443`)

## Capturing CPU Profiles

The easiest workflow is:

1. Run a profiling build of the server: `just skit-profiling serve`
2. Run a load test preset in another terminal (examples above)
3. Fetch profiles:
   - Top view: `just profile-top 30`
   - Web UI: `just profile-web 30`

`profile-*` commands require Go (`go tool pprof`).

## What Each Preset Targets

### `lt-oneshot-http-passthrough`

Targets request handling and oneshot overhead:

- Multipart parsing and streaming input
- Pipeline compilation/validation
- Graph wiring/spawn + channel plumbing

### `lt-oneshot-opus-transcode-fast`

Targets codec throughput:

- `containers::ogg::demuxer` (parsing)
- `audio::opus::{decoder,encoder}`

This intentionally runs “as fast as possible” (no pacer), so it’s useful for CPU profiling and throughput regressions.

### `lt-dynamic-tune-heavy`

Targets control-plane churn:

- Session creation churn (up to `dynamic.session_count`)
- Control WebSocket tuning rate (`dynamic.tune_interval_ms`)
- Parameter updates to many `audio::gain` nodes

### `lt-dynamic-moq-fanout`

Targets MoQ transport + data plane in dynamic sessions:

- One broadcaster session publishes to `input`
- Many subscriber sessions subscribe/transcode/publish

## Writing Your Own Config

Configs are TOML and validated by `skit-cli` before running:

- Pick a scenario: `test.scenario = "oneshot" | "dynamic" | "mixed"`
- Point to a pipeline YAML for that scenario
- Adjust `oneshot.concurrency` or `dynamic.session_count`

See `apps/skit-cli/src/load_test/config.rs` for the full schema.

## Tips

- For profiling, keep logging quiet (e.g. `RUST_LOG=warn`) to avoid measuring log formatting instead of pipeline CPU.
- For dynamic tests, use `--cleanup` when you want sessions deleted at the end: `just lt-dynamic-cleanup`.
- Prefer small input files for high-throughput profiling (e.g. `samples/audio/system/speech_2m.opus`) and larger files for sustained steady-state load.
