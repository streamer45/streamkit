---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Development Workflow
description: Build, test, and iterate on StreamKit locally
---

StreamKit standardizes local development through `just` (see `justfile` in the repo root).

## Common Commands

```bash
just build-ui  # Build embedded web UI (ui/dist)
just build-skit # Release server build
just build     # Full build: server + UI + plugins
just test      # Rust + UI test suites
just dev       # Server + UI hot reload
just lint      # Lint Rust, UI, and plugins
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
