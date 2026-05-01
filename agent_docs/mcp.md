<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# StreamKit MCP Server

StreamKit embeds a [Model Context Protocol](https://modelcontextprotocol.io/)
server that exposes the full control plane — node discovery, pipeline
validation, session lifecycle, sample browsing, and diagnostics — as MCP
tools, prompts, and resources.

## Transports

| Transport | Endpoint | Auth | Use case |
|-----------|----------|------|----------|
| Streamable HTTP | `POST /api/v1/mcp` | Bearer token (same as REST API) | Remote agents, multi-tenant |
| STDIO | `skit mcp` | None (implicit admin) | Local agents (Claude Desktop, Cursor, Devin) |

### Enabling

In `skit.toml`:

```toml
[mcp]
enabled = true
# endpoint = "/api/v1/mcp"    # default
# allowed_hosts = []           # DNS rebinding protection (optional)
```

Or via environment: `STREAMKIT_MCP__ENABLED=true`.

For STDIO, just run `skit mcp` — no config needed.

## Tools (16)

All tools follow the same pattern: `extract_auth() → permission check →
delegate to shared helper → result`.

### Discovery

| Tool | Description | Permission |
|------|-------------|------------|
| `list_nodes` | All node definitions (kind, schema, pins, categories) | any role |
| `get_node_definition` | Single node definition by kind | `is_node_allowed` + `is_plugin_allowed` |
| `list_plugins` | Installed plugins (kind, version, type) | `is_plugin_allowed` filter |
| `list_samples` | Sample pipelines with mode filter (`oneshot`/`dynamic`) | `list_samples` |
| `get_server_info` | Server version, features, limits, plugin count | non-viewer role |

### Pipeline validation

| Tool | Description | Permission |
|------|-------------|------------|
| `validate_pipeline` | Validate YAML without creating a session | `create_sessions` |
| `generate_oneshot_command` | Generate `curl` or `skit-cli` command for oneshot execution | `create_sessions` |

### Session lifecycle

| Tool | Description | Permission |
|------|-------------|------------|
| `create_session` | Create dynamic session from YAML | `create_sessions` |
| `list_sessions` | List active sessions | `list_sessions` |
| `get_pipeline` | Full pipeline state (nodes, connections, states) | `list_sessions` |
| `destroy_session` | Stop and remove a session | `destroy_sessions` |

### Live mutation

| Tool | Description | Permission |
|------|-------------|------------|
| `validate_batch` | Dry-run batch mutations (addnode, connect, etc.) | `modify_sessions` |
| `apply_batch` | Apply batch mutations atomically | `modify_sessions` |
| `tune_node` | Send control message (e.g. UpdateParams) to a node | `modify_sessions` |
| `update_pipeline` | Diff new YAML against running session and apply minimal batch ops | `modify_sessions` |

### Diagnostics

| Tool | Description | Permission |
|------|-------------|------------|
| `get_logs` | Recent server log lines with level/text filter | `access_all_sessions` |

## Prompts (2)

| Prompt | Description | Args |
|--------|-------------|------|
| `design_pipeline` | Node catalog + YAML format + connection rules | `description` (optional) |
| `debug_pipeline` | Pipeline state, node states, diagnostic checklist | `session_id` (required) |

## Resources (3 handlers)

Resources expose sample pipelines as browsable `streamkit://` URIs.

| Handler | URI pattern | Permission |
|---------|-------------|------------|
| `list_resources` | `streamkit://samples/{mode}/{id}` | `list_samples` |
| `read_resource` | Returns YAML content for a URI | `read_samples` |
| `list_resource_templates` | URI template schema | none (static metadata) |

Resource URIs follow `streamkit://samples/{subdir}/{id}` where subdir is
`oneshot`, `dynamic`, `demo`, or `user` (the subdirectory the sample lives in).

Note: `SamplePipeline.mode` is always `"oneshot"` or `"dynamic"` (from the
pipeline's YAML `mode:` field). The `demo`/`user` prefixes in resource URIs
refer to the *subdirectory*, not the pipeline mode.

## Permissions

The MCP server reuses StreamKit's role-based permission system. Key
permissions relevant to MCP:

| Permission | Controls |
|------------|----------|
| `create_sessions` | Creating sessions, validating pipelines |
| `list_sessions` | Listing sessions, getting pipeline state |
| `modify_sessions` | Batch mutations, tuning nodes |
| `destroy_sessions` | Destroying sessions |
| `list_samples` | Listing sample pipelines, listing resources |
| `read_samples` | Reading sample YAML, reading resources |
| `access_all_sessions` | Accessing server logs |
| `is_node_allowed(kind)` | Per-node access control |
| `is_plugin_allowed(kind)` | Per-plugin access control |

## Agent Usage Patterns

### Design a pipeline from scratch

```
1. prompts/get  → design_pipeline (with optional description)
2. tools/call   → validate_pipeline (check the YAML)
3. tools/call   → create_session (deploy it)
```

### Browse and use sample pipelines

```
1. resources/list        → discover available samples
2. resources/read        → fetch YAML for a sample
3. tools/call            → validate_pipeline (customize and validate)
4. tools/call            → create_session (deploy)
```

### Debug a running session

```
1. tools/call   → list_sessions (find the session)
2. prompts/get  → debug_pipeline (get diagnostics)
3. tools/call   → get_logs (check server logs for errors)
4. tools/call   → get_pipeline (inspect full state)
```

### Mutate a running session

```
1. tools/call   → validate_batch (dry-run the changes)
2. tools/call   → apply_batch (apply atomically)
3. tools/call   → tune_node (adjust parameters at runtime)
```

### Apply a full YAML update to a running session

```
1. tools/call   → update_pipeline (diffs YAML against current state, applies batch ops)
```

`update_pipeline` is a higher-level alternative to `validate_batch` +
`apply_batch`.  It takes the desired pipeline YAML and the session ID,
computes the diff (nodes added/removed, connections added/removed), and
applies the changes as batch operations.  Use it when the agent reasons
about pipeline YAML rather than individual graph mutations.

## Code Layout

```
apps/skit/src/mcp/
  mod.rs       Tool implementations, resource handlers, auth, helpers
  prompts.rs   Prompt definitions and content builders
  oneshot.rs   Oneshot command generation logic
```

## Testing

MCP integration tests: `cargo test -p streamkit-server --features mcp -- mcp_`

Tests cover all tools, prompts, resources, permission enforcement, STDIO
transport, and path traversal validation. See
`apps/skit/tests/mcp_integration_test.rs`.
