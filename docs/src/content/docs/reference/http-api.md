---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: HTTP API
description: REST endpoints for sessions, schemas, plugins, and oneshot processing
---

Base URL (default): `http://127.0.0.1:4545`

## Authentication

When built-in auth is enabled, all `/api/v1/*` endpoints require authentication (except `/healthz` and `/health`).

- Non-browser clients: `Authorization: Bearer <token>`
- Browsers: log in via `/login` (StreamKit stores the JWT in an HttpOnly cookie)

Example:

```bash
TOKEN="$(skit auth print-admin-token --raw)"
curl -H "Authorization: Bearer $TOKEN" http://127.0.0.1:4545/api/v1/config
```

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
- One or more media fields: names must match `streamkit::http_input` nodes

**Max body size**: Configurable via `[server].max_body_size` (default: 100 MB).

If one or more media fields are provided, the pipeline must include `streamkit::http_input` nodes to receive them. Each `http_input` can declare:

- `field`: single field name (default `media` when only one http_input exists, otherwise the node id)
- `required`: whether the field must be present (default `true`)
- `fields`: list of field entries (string or `{ name, required }`), which exposes one output pin per entry so each upload can be routed independently. When `fields` is set, only the listed fields are accepted; the legacy `media` field is disabled. `field` and `fields` are mutually exclusive.

Unexpected fields cause a `400`, and missing required fields time out.

Example (dual upload mixing sample, real assets + paced playback):

```bash
curl --no-buffer \
  -F config=@samples/pipelines/oneshot/dual_upload_mixing.yml \
  -F track_a=@samples/audio/system/speech_2m.opus \
  -F "track_b=@samples/audio/system/THE LADY IS A TRAMP.opus" \
  http://127.0.0.1:4545/api/v1/process | ffplay -nodisp -autoexit -f webm -i -
```

If no uploads are needed, `streamkit::http_input` can still be used as a trigger (with empty body) or the pipeline can rely solely on `core::file_reader`. Both nodes can be used together (e.g., mixing uploaded audio with a local file). In all cases, `streamkit::http_output` is required.

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

## Marketplace

Marketplace browsing (admin-only):

- `GET /api/v1/marketplace/registries`
- `GET /api/v1/marketplace/plugins?registry=<id>&q=<query>`
- `GET /api/v1/marketplace/plugins/{plugin_id}?registry=<id>&version=<version>`

Marketplace URL security defaults:

- HTTPS required, localhost/private/link-local/multicast hosts blocked
- same-origin enforcement for manifest/signature/bundle URLs is optional
- allowlists never bypass HTTPS or host/IP blocking
- redirects are validated per-hop, so allowlist must cover every host in the chain (e.g. GitHub
  Releases uses `github.com` plus `objects.githubusercontent.com` or `release-assets.githubusercontent.com`)

Install jobs:

- `POST /api/v1/plugins/install`
  - Body: `{ "registry": "...", "plugin_id": "...", "version"?: "...", "install_models"?: bool, "model_ids"?: string[] }`
- `GET /api/v1/jobs/{job_id}`
- `POST /api/v1/jobs/{job_id}/cancel`

Example job response:

```json
{
  "status": "running",
  "started_at_ms": 1730000000000,
  "updated_at_ms": 1730000005000,
  "summary": "Downloading bundle",
  "steps": [
    {
      "name": "download_bundle",
      "status": "running",
      "progress": {
        "bytes_done": 1048576,
        "bytes_total": 2097152
      }
    }
  ]
}
```

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

## Auth Tokens

When built-in auth is enabled, the following endpoints are available under `/api/v1/auth`:

- `POST /api/v1/auth/login` — Exchange a token for an HttpOnly session cookie
- `POST /api/v1/auth/logout` — Clear the session cookie
- `GET /api/v1/auth/me` — Return the current user's identity and role
- `POST /api/v1/auth/tokens` — Mint a new API token (admin only)
- `GET /api/v1/auth/tokens` — List minted tokens (admin only)
- `DELETE /api/v1/auth/tokens/{jti}` — Revoke a token by JTI (admin only)
- `POST /api/v1/auth/reload-keys` — Reload signing keys from disk (admin only)
- `POST /api/v1/auth/moq-tokens` — Mint a MoQ/WebTransport token (admin only; requires `moq` feature)

## Logs

- `GET /api/v1/logs` — Fetch recent log entries
- `GET /api/v1/logs/stream` — SSE stream of log entries in real time

## Image Assets

- `GET /api/v1/assets/images` — List image assets
- `POST /api/v1/assets/images` — Upload an image asset (max 10 MB)
- `GET /api/v1/assets/images/file/{scope}/{id}` — Serve an image file
- `DELETE /api/v1/assets/images/{id}` — Delete a user image asset

Allowed formats: `png`, `jpg`, `jpeg`, `webp`, `gif`, `svg`, `svgz`.

## Font Assets

- `GET /api/v1/assets/fonts` — List font assets
- `POST /api/v1/assets/fonts` — Upload a font asset (max 10 MB)
- `GET /api/v1/assets/fonts/file/{scope}/{id}` — Serve a font file
- `DELETE /api/v1/assets/fonts/{id}` — Delete a user font asset

## Plugin Assets

Plugin-declared assets (e.g. Slint UI files) are managed through a generic asset type system.

- `GET /api/v1/asset-types` — List registered asset types (from plugin manifests)
- `GET /api/v1/assets/plugin/{type_id}` — List assets for a type
- `POST /api/v1/assets/plugin/{type_id}` — Upload an asset (max 100 MB)
- `DELETE /api/v1/assets/plugin/{type_id}/{id}` — Delete an asset
- `GET /api/v1/assets/plugin/{type_id}/file/{scope}/{id}` — Serve / update an asset file

## WebSocket Control Plane

- `GET /api/v1/control` — WebSocket upgrade for session control

See the [WebSocket API reference](/reference/websocket-api/) for the message protocol.

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

### MCP (Model Context Protocol)

Requires: `--features mcp` and `[mcp].enabled = true` in config.

- `POST /api/v1/mcp` (default path; configurable via `[mcp].endpoint`) — Streamable HTTP transport for MCP clients

The MCP endpoint uses bearer token auth (same as REST API). For STDIO transport, use `skit mcp` instead.

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
