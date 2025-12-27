---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Configuration
description: skit.toml and environment overrides
---

StreamKit loads configuration in this order:

1. Built-in defaults
2. A TOML file (default: `skit.toml` if present)
3. Environment variables (override file + defaults)

## Generate a Config File

```bash
# Generate with all defaults
skit config default > skit.toml

# Output JSON schema (for editor autocomplete)
skit config schema > skit-schema.json
```

## Environment Variables

Environment variables are prefixed with `SK_` and use `__` to separate nested fields:

```bash
export SK_SERVER__ADDRESS=127.0.0.1:4545
export SK_PLUGINS__DIRECTORY=.plugins
export SK_LOG__CONSOLE_LEVEL=debug
export SK_TELEMETRY__ENABLE=false
```

---

## `[server]`

HTTP server configuration.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `address` | string | `127.0.0.1:4545` | Bind address (`host:port`) |
| `samples_dir` | string | `./samples/pipelines` | Directory for sample pipelines served by the UI |
| `max_body_size` | int | `104857600` | Max request body size in bytes (default: 100MB) |
| `base_path` | string? | `null` | Base path for subpath deployments (injects `<base>` into HTML) |
| `tls` | bool | `false` | Enable TLS |
| `cert_path` | string | `""` | Path to TLS certificate |
| `key_path` | string | `""` | Path to TLS private key |
| `moq_gateway_url` | string? | `null` | (MoQ builds) WebTransport gateway URL for the frontend; can be overridden with `SK_SERVER__MOQ_GATEWAY_URL` |
| `moq_address` | string? | `127.0.0.1:4545` | (Reserved) MoQ/WebTransport bind address (current builds listen on `[server].address`) |

**CORS settings** (`[server.cors]`):

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `allowed_origins` | string[] | `["http://localhost:*", ...]` | Allowed origins (supports wildcards) |

## `[plugins]`

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `directory` | string | `.plugins` | Plugin base directory |
| `allow_http_management` | bool | `false` | Allow plugin upload/delete via HTTP APIs (enable only in trusted environments) |

Plugins are stored in subfolders: `native/` for `.so`/`.dylib`/`.dll`, `wasm/` for `.wasm`.

## `[resources]`

Resource management for ML models and shared resources.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `keep_models_loaded` | bool | `true` | Keep loaded models in memory until explicit unload |
| `max_memory_mb` | int? | `null` | Memory limit for cached resources (LRU eviction when exceeded; only applies when `keep_models_loaded = false`) |

**Pre-warming** (`[resources.prewarm]`):

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | bool | `false` | Pre-warm configured plugins at startup |
| `plugins` | array | `[]` | List of plugins to pre-warm |

Each plugin in `plugins[]`:
- `kind` (string): plugin kind, e.g. `plugin::native::whisper`
- `params` (object?): params for the warmup instance
- `fallback_params` (object?): fallback if primary params fail (e.g., GPU → CPU)

## `[permissions]`

Role-based access control. StreamKit does not implement authentication—use a reverse proxy or auth layer.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `default_role` | string | `admin` | Role for unauthenticated requests |
| `role_header` | string? | `null` | Trusted header for role selection (only behind a proxy) |
| `allow_insecure_no_auth` | bool | `false` | Allow binding to a non-loopback address without a trusted role header (unsafe) |
| `max_concurrent_sessions` | int? | `null` | Global limit for dynamic sessions |
| `max_concurrent_oneshots` | int? | `null` | Global limit for oneshot requests |
| `roles` | map | see below | Role name → permissions |

Role resolution order:
1. Trusted header (`[permissions].role_header`) if configured
2. `SK_ROLE` environment variable
3. `[permissions].default_role`

**Role permissions** (`[permissions.roles.<name>]`):

> [!NOTE]
> Role permissions are **deny-by-default**. If you define a role in `skit.toml`, any permission you omit defaults to `false`.

| Option | Type | Default (admin) | Description |
|--------|------|-----------------|-------------|
| `create_sessions` | bool | `true` | Can create new sessions |
| `destroy_sessions` | bool | `true` | Can destroy sessions |
| `modify_sessions` | bool | `true` | Can add/remove nodes in sessions |
| `tune_nodes` | bool | `true` | Can tune node parameters |
| `list_sessions` | bool | `true` | Can list sessions |
| `list_nodes` | bool | `true` | Can view available nodes |
| `list_samples` | bool | `true` | Can list sample pipelines |
| `read_samples` | bool | `true` | Can read sample pipeline YAML |
| `write_samples` | bool | `true` | Can save/update user pipelines |
| `delete_samples` | bool | `true` | Can delete user pipelines |
| `access_all_sessions` | bool | `true` | Can access any user's sessions |
| `load_plugins` | bool | `true` | Can upload plugins |
| `delete_plugins` | bool | `true` | Can delete plugins |
| `upload_assets` | bool | `true` | Can upload audio assets |
| `delete_assets` | bool | `true` | Can delete audio assets |
| `allowed_samples` | string[] | `["*"]` | Allowed sample paths (globs), relative to `[server].samples_dir` (e.g. `oneshot/*.yml`) |
| `allowed_nodes` | string[] | `["*"]` | Allowed node types (wildcards) |
| `allowed_plugins` | string[] | `["*"]` | Allowed plugin names (wildcards) |
| `allowed_assets` | string[] | `["*"]` | Allowed asset paths (globs) |

## `[security]`

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `allowed_file_paths` | string[] | `["samples/**"]` | Allowed paths for `core::file_reader` (globs) |
| `allowed_write_paths` | string[] | `[]` | Allowed paths for `core::file_writer` (globs). Default deny (empty) |

## `[script]`

Configuration for the `core::script` node (requires `script` feature).

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `default_timeout_ms` | int | `100` | Per-packet script timeout |
| `default_memory_limit_mb` | int | `64` | QuickJS memory limit |
| `global_fetch_allowlist` | array | `[]` | Allowlist for `fetch()` calls |
| `secrets` | map | `{}` | Named secrets from environment |

**Fetch allowlist** (`global_fetch_allowlist[]`):
- `url` (string): wildcard URL pattern, e.g. `https://api.example.com/*`
- `methods` (string[]): allowed HTTP methods, e.g. `["GET", "POST"]`

**Secrets** (`secrets.<name>`):
- `env` (string): environment variable name
- `type` (string): `url` | `token` | `apikey` | `string`
- `description` (string): optional description

## `[engine]`

Pipeline execution tuning for **dynamic sessions** (long-running pipelines created via the sessions API).

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `profile` | string? | `null` | Preset tuning: `low-latency` \| `balanced` \| `high-throughput` (explicit capacities override the preset) |
| `packet_batch_size` | int | `32` | Batch size for processing (higher = throughput, lower = responsiveness) |
| `node_input_capacity` | int? | `null` | Per-node input buffer (default: 128 packets) |
| `pin_distributor_capacity` | int? | `null` | Buffer between outputs and distributors (default: 64 packets) |

For low-latency streaming, consider `node_input_capacity: 8-16` and `pin_distributor_capacity: 4-8`.

### `[engine.oneshot]`

Configuration for **oneshot pipelines** (HTTP batch processing via `/api/v1/process`).

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `packet_batch_size` | int | `32` | Batch size for processing |
| `media_channel_capacity` | int? | `256` | Buffer size between nodes (packets) |
| `io_channel_capacity` | int? | `16` | HTTP I/O stream buffer (packets) |

Oneshot pipelines use larger defaults than dynamic sessions for batch efficiency.

### `[engine.advanced]`

Advanced internal buffer configuration for codec and container nodes. **Only modify if you understand the latency/throughput implications.**

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `codec_channel_capacity` | int? | `32` | Async/blocking handoff in codec nodes (e.g., Opus) |
| `stream_channel_capacity` | int? | `8` | Bytes→blocking handoff for streaming demux/decode tasks |
| `demuxer_buffer_size` | int? | `65536` | OGG demuxer duplex buffer (bytes) |
| `moq_peer_channel_capacity` | int? | `100` | (MoQ builds) MoQ peer internal channels (packets) |

See the [Performance Tuning](/guides/performance) guide for when to adjust these values.

## `[log]`

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `console_enable` | bool | `true` | Enable console logging |
| `console_level` | string | `info` | Console log level: `debug` \| `info` \| `warn` \| `error` |
| `file_enable` | bool | `true` | Enable file logging |
| `file_level` | string | `info` | File log level |
| `file_path` | string | `./skit.log` | Log file path |
| `file_format` | string | `text` | File log format: `text` (fast) or `json` (structured; higher CPU) |

## `[telemetry]`

OpenTelemetry and observability.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | bool | `true` | Enable telemetry |
| `otlp_endpoint` | string? | `null` | OTLP endpoint URL |
| `otlp_headers` | map | `{}` | Headers for OTLP requests |
| `tracing_enable` | bool | `false` | Enable OpenTelemetry tracing (spans) export |
| `otlp_traces_endpoint` | string? | `null` | OTLP endpoint for trace export (e.g., `http://localhost:4318/v1/traces`) |
| `tokio_console` | bool | `false` | Enable tokio-console (requires `tokio-console` feature) |

---

## Security Notes

- **File access**: Pipelines using `core::file_reader` are restricted by `[security].allowed_file_paths`.
- **File writes**: Pipelines using `core::file_writer` are restricted by `[security].allowed_write_paths` (default deny).
- **WebSocket Origin**: Browser WebSocket connections to `/api/v1/control` must match `[server.cors].allowed_origins`.
- **Role headers**: Only enable `[permissions].role_header` behind a trusted reverse proxy that strips incoming headers with the same name.
- **Default role**: For production, set `default_role` to a least-privileged role and use an auth layer.
