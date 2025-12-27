---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: HTTP API
description: REST endpoints for sessions, schemas, plugins, and oneshot processing
---

Base URL (default): `http://127.0.0.1:4545`

## Health

- `GET /healthz`
- `GET /health`

Lightweight readiness endpoint used by the official Docker images.

## Config (UI bootstrap)

`GET /api/v1/config`

Used by the UI and as a simple health check.

## Permissions

`GET /api/v1/permissions`

Returns the active role and allowed capabilities for the request.

## Node + Packet Schemas

- `GET /api/v1/schema/nodes`
- `GET /api/v1/schema/packets`

## Sessions (dynamic pipelines)

Create a session from YAML:

- `POST /api/v1/sessions`
- Body: `{ "name"?: string, "yaml": string }`

List sessions:

- `GET /api/v1/sessions`

Fetch the current pipeline (includes runtime node state):

- `GET /api/v1/sessions/{id-or-name}/pipeline`

Destroy a session:

- `DELETE /api/v1/sessions/{id-or-name}`
- Returns: `{ "session_id": string }`

## Oneshot Processing

`POST /api/v1/process` accepts multipart:

- `config`: pipeline YAML (required; must be the first field)
- `media`: optional binary media payload

**Max body size**: Configurable via `[server].max_body_size` (default: 100 MB).

If `media` is provided, the pipeline must include `streamkit::http_input` and typically ends with `streamkit::http_output`.

If `media` is not provided, the pipeline must include `core::file_reader` and must not include `streamkit::http_input`. In both cases, `streamkit::http_output` is required.

> [!NOTE]
> `streamkit::http_input` and `streamkit::http_output` are **oneshot-only marker nodes**. They are available in schema discovery, but they cannot be used in dynamic sessions.

## Plugins

- `GET /api/v1/plugins` (list)
- `POST /api/v1/plugins` (upload; multipart field name `plugin`)
- `DELETE /api/v1/plugins/{kind}` (unload and optionally delete)

By default, plugin upload/delete APIs are disabled; enable them with `[plugins].allow_http_management = true` and restrict access to trusted callers.

**DELETE query parameters:**

| Parameter | Default | Description |
|-----------|---------|-------------|
| `keep_file` | `false` | If `true`, keeps the plugin file on disk but unloads it from memory. If `false` (default), deletes both the file and unloads from memory. |

Uploaded plugins are registered under:

- `plugin::native::<kind>` for native libraries
- `plugin::wasm::<kind>` for WASM components

## Sample Pipelines

Sample pipelines are used by the UI. They live under `[server].samples_dir` (default: `./samples/pipelines`). Permission allowlists for samples (`allowed_samples`) are evaluated against paths relative to that directory (e.g. `oneshot/*.yml`).

- `GET /api/v1/samples/oneshot` (list)
- `GET /api/v1/samples/oneshot/{id}` (fetch YAML)
- `POST /api/v1/samples/oneshot` (save)
- `DELETE /api/v1/samples/oneshot/{id}` (delete user samples only)
- `GET /api/v1/samples/dynamic` (list dynamic samples)

**POST body (`SavePipelineRequest`):**

```json
{
  "name": "my-pipeline",
  "description": "A sample pipeline",
  "yaml": "mode: oneshot\nsteps: ...",
  "overwrite": false,
  "is_fragment": false
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Pipeline filename (without extension) |
| `description` | string | Yes | Human-readable description |
| `yaml` | string | Yes | Pipeline YAML content |
| `overwrite` | bool | No | Overwrite existing file (default: `false`) |
| `is_fragment` | bool | Yes | If `true`, stores as a partial pipeline fragment rather than a complete pipeline |

**Max body size**: 1 MB (hardcoded).

## Audio Assets

Audio assets are served from `samples/audio/`:

- System assets: `samples/audio/system/` (read-only)
- User uploads: `samples/audio/user/`

Endpoints:

- `GET /api/v1/assets/audio` (list)
- `POST /api/v1/assets/audio` (upload; multipart with a filename)
- `DELETE /api/v1/assets/audio/{id}` (delete user assets only)

**Max upload size**: 100 MB.

**Response fields (`AudioAsset`):**

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Unique asset identifier (filename, including extension) |
| `name` | string | Display name |
| `path` | string | Server-relative path suitable for `core::file_reader` (e.g., `samples/audio/system/foo.wav`) |
| `format` | string | Audio format (e.g., `ogg`, `wav`) |
| `size_bytes` | number | File size in bytes |
| `license` | string? | Optional license information |
| `is_system` | bool | `true` for system assets, `false` for user uploads |

## Feature-Gated Endpoints

The following endpoints require specific build features and may not be available in all builds (including some Docker images).

### Profiling

Requires: `--features profiling`

- `GET /api/v1/profile/cpu?duration_secs=30&format=flamegraph|protobuf&frequency=99`
- `GET /api/v1/profile/heap`

These endpoints are restricted to roles with admin-level access (`access_all_sessions = true`).

If profiling is not enabled, these endpoints return `501 Not Implemented`.

### MoQ (Media over QUIC)

Requires: `--features moq`

- `GET /api/v1/moq/fingerprints` (WebTransport certificate fingerprints)
- `GET /certificate.sha256` (first fingerprint, plain text)

## Error Responses

HTTP errors are returned as plain text with appropriate status codes:

| Status | Meaning |
|--------|---------|
| `400 Bad Request` | Invalid request body, YAML syntax error, or invalid pipeline |
| `403 Forbidden` | Permission denied for the requested operation |
| `404 Not Found` | Session, asset, or resource not found |
| `429 Too Many Requests` | Global session or oneshot limit reached |
| `500 Internal Server Error` | Server-side error during processing |
| `501 Not Implemented` | Feature not enabled in this build |
