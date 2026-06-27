---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Security Configuration
description: File access, origin checks, plugin management, and other guardrails
---

This guide covers security-sensitive configuration in `skit.toml` beyond authentication and role setup.

## Runtime plugin management gate

Even when a role has `load_plugins` / `delete_plugins`, StreamKit can globally disable runtime plugin management:

```toml
[plugins]
allow_http_management = false  # default
```

Set it to `true` only in trusted environments (local development or behind strong auth).

## File system access

The `core::file_reader` node can read files from disk. Restrict this with allowlists:

```toml
[security]
allowed_file_paths = [
  "samples/**",      # Allow reading samples
  "/data/audio/**",  # Allow specific data directory
]
```

Paths use glob patterns. Files outside these patterns cannot be read. Relative
patterns (and the node `path`/`script_path` fields they guard) are resolved
against `[server].asset_root` — the same path-space the file nodes read from —
so validation and runtime always agree. Node paths must be relative to
`asset_root`; absolute paths and `..` components are rejected.

### File writes (core::file_writer)

The `core::file_writer` node can write files to disk. For safety, writes are disabled by default.

```toml
[security]
# Default: [] (deny all writes)
allowed_write_paths = [
  "./output/**",
  "/data/exports/**",
]
```

This applies to both the HTTP oneshot endpoint and the WebSocket control plane.

## Origin checks (browser safety)

To mitigate cross-site request attacks in browsers, StreamKit validates `Origin` against
`[server.cors].allowed_origins`:

- **WebSocket**: `/api/v1/control`
- **HTTP**: mutating `/api/*` requests (e.g. `POST /api/v1/process`)

Requests without an `Origin` header (CLI/tools) are still allowed.

## Profiling endpoints

If you build with `--features profiling`, the server exposes `/api/v1/profile/cpu` and
`/api/v1/profile/heap`. These endpoints are restricted to roles with admin-level access
(`access_all_sessions = true`) and should not be exposed to untrusted clients.

## Script node controls

The `core::script` node has allowlists and resource limits for safe `fetch()` usage and secrets
injection. See the [Script Node Guide](/guides/script-node/) for the full model, including
`global_fetch_allowlist`, secret mapping, and runtime limits.

## Plugin security model

StreamKit supports two plugin types with different security properties:

- **Native plugins** run in-process with full access. Only load trusted code.
- **WASM plugins** run in a sandboxed environment with no filesystem or network access by default.

See [Writing Plugins](/guides/writing-plugins/) for details and recommended practices.

## Production baseline checklist

See the [Security](/guides/security/#baseline-checklist) page for a consolidated production checklist.
