---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Authorization & Roles
description: Role-based access control (RBAC) and permissions
---

Authorization determines what a caller can do. StreamKit uses role-based access control (RBAC): every request resolves to a role, and that role's permissions gate the API, Web UI actions, and runtime management features.

## How roles are resolved

- **Built-in auth enabled**: the role comes from the JWT (`role` claim) minted by StreamKit.
- **Built-in auth disabled**: the role is resolved in this order:
  1. Trusted header (`[permissions].role_header`), if configured
  2. `SK_ROLE` environment variable
  3. `[permissions].default_role`

> [!CAUTION]
> Only enable `role_header` behind a trusted reverse proxy that strips any incoming header with the same name. Otherwise, clients can impersonate any role.

If you disable built-in auth while binding to a non-loopback address, StreamKit refuses to start unless you explicitly opt in:

```toml
[permissions]
allow_insecure_no_auth = false # default
```

## Configure roles

```toml
[permissions]
# Role assigned to unauthenticated requests
default_role = "viewer"

# Trusted header for role selection (only behind a reverse proxy)
role_header = "X-StreamKit-Role"

# Safety gate: refuse to bind to non-loopback without a trusted auth layer.
# Set this to true only if the server is reachable exclusively by trusted clients.
allow_insecure_no_auth = false # default

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
allowed_nodes = ["audio::*", "core::*"] # Restrict to audio and core nodes
allowed_assets = ["*"]
```

> [!NOTE]
> Role permissions are deny-by-default. If you define a custom role in `skit.toml`, any permission you omit defaults to `false`.

> [!NOTE]
> The built-in `user` role allows the safe HTTP sink/IO nodes — `transport::http::mse` (live-cast playback), `streamkit::http_input` and `streamkit::http_output` (oneshot request body/response) — but **not** `transport::http::fetcher`, which can fetch arbitrary URLs (SSRF risk). A trusted gateway that only serves or receives over the caller's own request therefore does not need `admin`.

## Example: Least-privilege gateway role

Trusted intermediaries (e.g. the `web-capture` or `speech-gateway` examples) build a small set of fixed pipelines and should run with a scoped token instead of `admin`. This role can create/destroy sessions and use the HTTP-transport sink/IO nodes plus a specific plugin, but cannot load/delete plugins or touch other users' sessions:

```toml
[permissions.roles.gateway]
create_sessions = true
destroy_sessions = true
modify_sessions = true
tune_nodes = true
list_sessions = true
list_nodes = true
access_all_sessions = false  # Only its own sessions
load_plugins = false
delete_plugins = false
upload_assets = false
delete_assets = false
allowed_nodes = [
  "transport::http::mse",   # serve live casts to the browser (MSE)
  "streamkit::http_input",  # oneshot request body
  "streamkit::http_output", # oneshot response
  "core::*",
]
allowed_plugins = ["plugin::native::servo"] # only what the gateway needs
```

## Permission reference

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

## Example: Multi-tenant setup

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
