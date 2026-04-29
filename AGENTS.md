<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# StreamKit Agent Notes

These notes apply to coding agents (Claude/Codex/Devin/etc.) contributing to
this repo. Agent-assisted contributions are welcome, but should be **supervised**
and **reviewed by a human** before merge.

## What is StreamKit

StreamKit is a self-hostable media processing server (Rust). A single binary
(`skit`) runs pipelines as a node graph (DAG) — via a web UI, YAML, or
WebSocket API. Two modes: **dynamic** (real-time, hot-reconfigurable) and
**oneshot** (stateless batch processing). See `agent_docs/architecture.md` for
the full architecture.

## Codebase Map

| Directory | Purpose |
|-----------|---------|
| `apps/skit/` | Server binary — HTTP/WS handlers, config, auth, plugins |
| `apps/skit-cli/` | CLI client binary (`streamkit-client`) |
| `crates/core/` | Shared traits/types — `ProcessorNode`, `Pin`, `Packet`, `NodeRegistry` |
| `crates/engine/` | Pipeline executor — graph builder, oneshot engine, dynamic actor |
| `crates/nodes/` | Built-in nodes: `audio::`, `video::`, `transport::`, `core::`, `containers::` |
| `crates/api/` | YAML pipeline parsing, WebSocket protocol, TS type generation |
| `crates/plugin-{native,wasm}/` | Plugin host adapters (FFI and WASM) |
| `sdks/plugin-sdk/` | Plugin SDKs for Rust, Go, and C |
| `ui/` | React 19 web UI (Vite + Bun) |
| `plugins/native/` | Official ML plugins (Whisper, Kokoro, NLLB, etc.) |
| `samples/` | Example pipelines (`dynamic/` and `oneshot/`), audio files, images, fonts, Slint files |
| `tests/` | Pipeline validation tests (oneshot pipeline smoke tests) |
| `e2e/` | Playwright end-to-end tests |
| `docs/` | Astro + Starlight docs site (sidebar in `docs/astro.config.mjs`) |

## Tech Stack

- **Rust** (version pinned in `rust-toolchain.toml`), tokio, axum, wgpu
- **UI:** React 19, TypeScript, Zustand, Jotai, React Query, Radix UI, React Flow
- **Build/tooling:** `just` (task runner), Bun (UI), sccache (Rust build cache)
- **Testing:** `cargo test` (Rust), Vitest (UI), Playwright (E2E)
- **Platform:** Linux x86_64

## Workflow

- Keep PRs focused and minimal.
- Run `just test` and `just lint` before submitting (or explain why you couldn't).
- Follow `CONTRIBUTING.md`: DCO sign-off (`git commit -s`), Conventional
  Commits, SPDX license headers on all new files.
- **Linting discipline**: Do not blindly suppress lint warnings with
  ignore/exception rules. Refactor instead. If a suppression is truly necessary,
  include a comment explaining the rationale.
- **UI tooling**: Use `bun install` / `bunx` / `bun run` — never npm or pnpm.

## Fix Root Causes, Not Symptoms

Prefer a clean change that takes longer over a brittle stack of patches that
ships sooner. If a feature requires defending against the same race in three
places, layering synchronous shadow refs over async state, or "preferring
draft over live" because two event sources disagree on timing — **stop and
reconsider the contract**, don't add a fourth patch.

Concrete signals you've crossed into workaround territory:

- You're adding a *timeout* to recover from a missing event.
- You need a synchronous shadow of state that already lives in an async store.
- You catch yourself writing "the X event doesn't actually mean X, it means
  *attempted* X, so we also have to listen for Y to know if it really
  happened."
- Tests are updated by adding sleeps or by relying on previously-broken
  behavior (e.g. an invalid value being silently accepted).
- The root cause is in a different layer than the one you're editing, and
  fixing it there would invalidate most of your patch.

When you spot this, surface the design issue to the user with a concrete
proposal — even if it's more invasive — *before* writing the patch. State
the tradeoff honestly: "this will take longer but produces something
durable; vs. this short-term fix has these specific brittleness costs."

Past incident worth remembering: the WebSocket `nodeadded` event used to fire
before plugin construction had even started. The UI accumulated 13 commits of
draft-state machinery (state-watchers, debounce timers, topology priority
hacks) trying to reconstruct "did the node actually get created?" from
out-of-band signals. The actual fix was a small server change — emit
`nodeadded` from the engine actor's success path instead of the WS handler —
which collapsed the UI back to the obvious cleanup. Code that exists to
paper over a broken contract should be deleted, not refined.

## Verification Commands

| Task | Command |
|------|---------|
| All lints | `just lint` |
| Rust lint only | `just lint-skit` (fmt + clippy with per-crate feature flags + license check) |
| UI lint only | `just lint-ui` (prettier + eslint + tsc) |
| All tests | `just test` |
| Rust tests | `cargo test --workspace` |
| UI tests | `just test-ui` |
| Perf regression tests | `just perf-ui` |
| E2E tests | `just e2e-external http://localhost:4545` (requires running server) |
| Unused code check | `just knip-ui` |
| Build everything | `just build` |

## Docker

- Official images: `Dockerfile` (CPU) and `Dockerfile.gpu` (GPU) via `.github/workflows/docker.yml`.
- Health endpoint: `/healthz` (also `/health`).
- Standard images do not bundle ML models or plugins — mount them at runtime.
  Demo images (`Dockerfile.demo`, tagged `-demo`) include bundled models and plugins.

## MCP (Model Context Protocol) Integration

StreamKit embeds an MCP server (`apps/skit/src/mcp/`) that exposes the
control plane as MCP tools, prompts, and resources. **Agents with MCP client
support can use this directly** — no REST/WebSocket code needed.

- **Endpoint:** `POST /api/v1/mcp` (Streamable HTTP) or `skit mcp` (STDIO)
- **Config:** `[mcp]` section in `skit.toml` — set `enabled = true`
- **Auth:** HTTP transport uses bearer tokens (same as REST API). STDIO is
  unauthenticated (admin-level, local-only).
- **Code:** `apps/skit/src/mcp/mod.rs` (tools + resources),
  `apps/skit/src/mcp/prompts.rs` (prompts)
- **Tests:** `apps/skit/tests/mcp_integration_test.rs`

See [`agent_docs/mcp.md`](agent_docs/mcp.md) for the full tool/prompt/resource
reference and usage patterns.

## Detailed Guides

Read the relevant guide **before** starting work in that area:

| Guide | When to read |
|-------|-------------|
| [`agent_docs/architecture.md`](agent_docs/architecture.md) | Understanding crate relationships, data flow, key abstractions |
| [`agent_docs/mcp.md`](agent_docs/mcp.md) | Using the MCP server — tools, prompts, resources, auth, permissions |
| [`agent_docs/ui-development.md`](agent_docs/ui-development.md) | Working on React UI — state management, component patterns |
| [`agent_docs/e2e-testing.md`](agent_docs/e2e-testing.md) | Running E2E tests, headless-browser pitfalls |
| [`agent_docs/render-performance.md`](agent_docs/render-performance.md) | Compositor perf profiling, render regression testing |
| [`agent_docs/adding-plugins.md`](agent_docs/adding-plugins.md) | Making a plugin official — full checklist |
| [`agent_docs/common-pitfalls.md`](agent_docs/common-pitfalls.md) | Known mistakes agents make — read this first if unsure |
| [`agent_docs/skills-setup.md`](agent_docs/skills-setup.md) | Install curated [skills.sh](https://skills.sh/) packages for React, Playwright, etc. |
