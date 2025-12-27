---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Security
description: Permissions, secrets, and plugin sandboxing
---

StreamKit provides several security mechanisms for production deployments.

## Runtime Plugin Management Gate

Even when a role has `load_plugins` / `delete_plugins`, StreamKit can globally disable runtime plugin management.

```toml
[plugins]
allow_http_management = false  # default
```

Set it to `true` only in trusted environments (e.g., local development, or behind an authenticated reverse proxy).

## Role-Based Permissions

StreamKit uses role-based access control (RBAC) to restrict what users can do. The built-in defaults assign unauthenticated requests the `admin` role (full access), but the official Docker images ship with `docker-skit.toml` which sets `default_role = "user"`.

StreamKit does not implement authentication. If you expose the server to untrusted clients, put it behind an authenticating reverse proxy (nginx/Caddy/etc) and configure a trusted role header.

### Configuring Roles

```toml
[permissions]
# Role assigned to unauthenticated requests
default_role = "viewer"

# Trusted header for role selection (only behind a reverse proxy)
role_header = "X-StreamKit-Role"

# Safety gate: refuse to bind to non-loopback without a trusted auth layer.
# Set this to true only if the server is reachable exclusively by trusted clients.
allow_insecure_no_auth = false  # default

# Global limits
max_concurrent_sessions = 10
max_concurrent_oneshots = 5

# Define roles
[permissions.roles.admin]
create_sessions = true
destroy_sessions = true
modify_sessions = true
tune_nodes = true
list_sessions = true
list_nodes = true
list_samples = true
read_samples = true
write_samples = true
delete_samples = true
access_all_sessions = true
load_plugins = true
delete_plugins = true
upload_assets = true
delete_assets = true
allowed_samples = ["*"]
allowed_nodes = ["*"]
allowed_plugins = ["*"]
allowed_assets = ["*"]

[permissions.roles.viewer]
create_sessions = false
destroy_sessions = false
modify_sessions = false
tune_nodes = false
list_sessions = true
list_nodes = true
list_samples = true
read_samples = true
write_samples = false
delete_samples = false
access_all_sessions = false
load_plugins = false
delete_plugins = false
upload_assets = false
delete_assets = false
allowed_samples = ["*"]
allowed_nodes = ["*"]
allowed_assets = ["*"]

[permissions.roles.operator]
create_sessions = true
destroy_sessions = true
modify_sessions = true
tune_nodes = true
list_sessions = true
list_nodes = true
list_samples = true
read_samples = true
write_samples = true
delete_samples = true
access_all_sessions = false  # Can only access own sessions
load_plugins = false
delete_plugins = false
upload_assets = true
delete_assets = true
allowed_samples = ["*"]
allowed_nodes = ["audio::*", "core::*"]  # Restrict to audio and core nodes
allowed_assets = ["*"]
```

### Role Selection

1. **Default role**: Applied when no role header is present
2. **Header-based**: When `role_header` is set, the specified header determines the role

> [!CAUTION]
> Only enable `role_header` behind a trusted reverse proxy that strips the header from client requests. Otherwise, clients can impersonate any role.

### Permission Reference

> [!NOTE]
> Role permissions are **deny-by-default**. If you define a custom role in `skit.toml`, any permission you omit defaults to `false`.

| Permission | Description |
|------------|-------------|
| `create_sessions` | Create new dynamic pipeline sessions |
| `destroy_sessions` | Destroy sessions |
| `modify_sessions` | Add/remove nodes and connections |
| `tune_nodes` | Update node parameters at runtime |
| `list_sessions` | View session list |
| `list_nodes` | View available node types |
| `list_samples` | List sample pipelines |
| `read_samples` | Read sample pipeline YAML |
| `write_samples` | Save/update user pipelines (writes to disk under `[server].samples_dir/user`) |
| `delete_samples` | Delete user pipelines |
| `access_all_sessions` | Access any user's sessions (vs only own) |
| `load_plugins` | Upload new plugins |
| `delete_plugins` | Remove plugins |
| `upload_assets` | Upload audio assets |
| `delete_assets` | Delete audio assets |
| `allowed_samples` | Glob patterns for allowed sample pipelines (paths are relative to `[server].samples_dir`) |
| `allowed_nodes` | Glob patterns for allowed node types |
| `allowed_plugins` | Glob patterns for allowed plugin names |
| `allowed_assets` | Glob patterns for allowed audio asset paths |

## File System Security

The `core::file_reader` node can read files from disk. Restrict this with:

```toml
[security]
allowed_file_paths = [
  "samples/**",      # Allow reading samples
  "/data/audio/**",  # Allow specific data directory
]
```

Paths use glob patterns. Files outside these patterns cannot be read.

### File Writes (core::file_writer)

The `core::file_writer` node can write files to disk. For safety, **writes are disabled by default**.

To enable file writes, explicitly allow destination paths:

```toml
[security]
# Default: [] (deny all writes)
allowed_write_paths = [
  "./output/**",
  "/data/exports/**",
]
```

This applies to both the HTTP oneshot endpoint and the WebSocket control plane.

## WebSocket Origin Checks

To mitigate Cross-Site WebSocket Hijacking (CSWSH) in browsers, StreamKit validates the WebSocket `Origin`
header for `/api/v1/control` against `[server.cors].allowed_origins`. Non-browser clients that don't send
`Origin` are still allowed.

## HTTP Origin Checks

For additional defense-in-depth in browser environments, StreamKit also validates the `Origin` header on
mutating `/api/*` requests (e.g. `POST /api/v1/process`) against `[server.cors].allowed_origins`.
This helps prevent cross-site attacks against local/self-hosted instances. Requests without an `Origin`
header (typical for CLI/tools) are still allowed.

## Profiling Endpoints

If you build with `--features profiling`, the server exposes `/api/v1/profile/cpu` and `/api/v1/profile/heap`.
These endpoints are restricted to roles with admin-level access (`access_all_sessions = true`) and should
not be exposed to untrusted clients.

## Script Node Security

The `core::script` node executes JavaScript. It has built-in security controls.

Notes:
- `fetch()` is controlled by a global server allowlist (empty allowlist blocks all).
- Allowlist checks are performed against the parsed URL host/path (not a raw string match).
- To reduce DoS risk from many concurrent `fetch()` calls, StreamKit limits in-flight `fetch()` operations globally. You can override the limit with `SK_SCRIPT_FETCH_MAX_INFLIGHT` (default: 16).
- Redirects are disabled for `fetch()` to avoid allowlist bypass and secret leakage.

### Fetch Allowlist

By default, scripts cannot make HTTP requests. Enable specific URLs:

```toml
[script]
default_timeout_ms = 100
default_memory_limit_mb = 64

# Allow API calls
[[script.global_fetch_allowlist]]
url = "https://api.example.com/*"
methods = ["GET", "POST"]

[[script.global_fetch_allowlist]]
url = "https://webhook.site/*"
methods = ["POST"]
```

### Secrets Management

Pass secrets to scripts without exposing them in pipeline YAML:

```toml
[script.secrets]
[script.secrets.OPENAI_KEY]
env = "OPENAI_API_KEY"
type = "apikey"
allowed_fetch_urls = ["https://api.openai.com/*"]
description = "OpenAI API key for completions"

[script.secrets.WEBHOOK_URL]
env = "WEBHOOK_URL"
type = "url"
description = "Webhook endpoint for notifications"
```

If you set `allowed_fetch_urls`, StreamKit only injects that secret into `fetch()` requests whose URL matches one of the patterns.

Secrets are not directly accessible from JavaScript. Instead, map them into HTTP headers for `fetch()`:

```yaml
mode: dynamic
steps:
  - kind: core::script
    params:
      headers:
        - secret: OPENAI_KEY
          header: Authorization
          template: "Bearer {}"
        - secret: WEBHOOK_URL
          header: X-Webhook-Url
      script: |
        function process(packet) {
          // fetch() will include the configured secret-backed headers
          const response = fetch("https://api.example.com/...");
          return packet;
        }
```

### Resource Limits

Each script execution has:
- **Timeout**: `default_timeout_ms` (default: 100ms per packet)
- **Memory**: `default_memory_limit_mb` (default: 64 MB)

These can be overridden per-node in the pipeline YAML.

## Plugin Security

StreamKit supports two plugin types with different security models.

### Native Plugins

Native plugins (`.so`/`.dylib`/`.dll`) run in-process with full access:

- **No sandbox**: Full system access
- **Trusted code only**: Only load plugins you trust
- **Use case**: Performance-critical, first-party plugins

### WASM Plugins

WASM plugins run in a sandboxed environment:

- **Sandboxed**: Cannot access filesystem, network, or system APIs
- **Capabilities**: Only exposed APIs (packet processing, logging)
- **Use case**: Third-party plugins, untrusted code

### Plugin Loading Permissions

Control who can load plugins:

```toml
[permissions.roles.operator]
load_plugins = false   # Cannot upload plugins
delete_plugins = false # Cannot remove plugins

[permissions.roles.admin]
load_plugins = true
delete_plugins = true
```

## Production Recommendations

### 1. Use a Reverse Proxy

Place StreamKit behind a reverse proxy (Caddy, nginx) for:
- TLS termination
- Authentication
- Rate limiting
- Role header injection

### 2. Restrict Default Role

```toml
[permissions]
default_role = "viewer"  # Not "admin"
```

### 3. Enable TLS

```toml
[server]
tls = true
cert_path = "/path/to/cert.pem"
key_path = "/path/to/key.pem"
```

### 4. Limit Concurrent Operations

```toml
[permissions]
max_concurrent_sessions = 10
max_concurrent_oneshots = 5
```

### 5. Restrict File Access

```toml
[security]
allowed_file_paths = ["samples/**"]  # Minimal access
```

### 6. Audit Script Allowlists

Only allow necessary URLs in `global_fetch_allowlist`. Start with none and add as needed.

## Example: Multi-Tenant Setup

```toml
[permissions]
default_role = "user"
role_header = "X-StreamKit-Role"
max_concurrent_sessions = 100

[permissions.roles.user]
create_sessions = true
destroy_sessions = true
modify_sessions = true
tune_nodes = true
list_sessions = true
list_nodes = true
access_all_sessions = false  # Only own sessions
load_plugins = false
delete_plugins = false
upload_assets = true
delete_assets = true
allowed_nodes = ["audio::*", "core::passthrough", "core::text_chunker"]
allowed_plugins = []  # No plugins

[permissions.roles.admin]
# Full access for administrators
create_sessions = true
destroy_sessions = true
modify_sessions = true
tune_nodes = true
list_sessions = true
list_nodes = true
access_all_sessions = true
load_plugins = true
delete_plugins = true
upload_assets = true
delete_assets = true
allowed_samples = ["*"]
allowed_nodes = ["*"]
allowed_plugins = ["*"]
```
