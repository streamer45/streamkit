---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Development Workflow
description: Build, test, and iterate on StreamKit locally
---

StreamKit standardizes local development through `just` (see `justfile` in the repo root).

## Prerequisites

`just skit`, `just build-skit`, and `just dev` compile the Rust server, which embeds the web UI at compile time. Run `just build-ui` first whenever `ui/dist/` is missing.

```bash
# Required once before compiling the server from a fresh checkout
just build-ui

# Required for hot reload (`just dev`)
cargo install cargo-watch
```

On Ubuntu/Debian, install `libvpx-dev pkg-config` before enabling VP9/video builds or running VP9 sample pipelines.

## Common Commands

```bash
just build-ui   # Build embedded web UI (ui/dist); run before compiling the server
just build-skit # Release server build
just build      # Full build: server + UI + plugins
just test       # Rust + UI test suites
just dev        # Server + UI hot reload (requires cargo-watch)
just lint       # Lint Rust, UI, and plugins
```

## Run the Server

```bash
just skit serve
```

To use a specific config file:

```bash
just skit '--config skit.toml serve'
```

## Run the Web UI (standalone)

```bash
just ui
```

## Regenerate TypeScript Types

When API/shared Rust types change:

```bash
just gen-types
```

This updates `ui/src/types/generated/api-types.ts`.

## Docs Site

```bash
just docs         # Start Starlight dev server
just build-docs   # Build production docs
just preview-docs # Preview production build
```
