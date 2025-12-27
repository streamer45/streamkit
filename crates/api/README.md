<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# StreamKit API

Shared, versioned API types for StreamKit (HTTP + WebSocket) plus generated TypeScript bindings.

## What Lives Here

- Core request/response/event types (`src/lib.rs`)
- YAML pipeline schema + compiler (`src/yaml.rs`)
- TypeScript bindings exported via `ts-rs` (committed under `api/bindings/` when generated)

## Regenerating TypeScript Types

From the repo root:

```bash
just gen-types
```

That runs `streamkit-api`'s `generate-ts-types` binary and updates the UI-consumable types.
