<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# MCP Usage

StreamKit has an embedded MCP server that exposes the full control plane.
Agents with MCP client support (like Devin) can interact with StreamKit
directly via MCP instead of writing REST/WebSocket code.

## Setup

### HTTP Transport (remote/multi-tenant)
1. Enable MCP in `skit.toml`: `[mcp] enabled = true`
2. Start the server: `just skit` (or with env: `STREAMKIT_MCP__ENABLED=true just skit`)
3. Endpoint: `POST http://localhost:4545/api/v1/mcp`
4. Auth: use a bearer token (same tokens as the REST API)

### STDIO Transport (local agents)
1. Run `skit mcp` — no config or auth needed
2. The caller gets implicit admin permissions
3. Use this for Claude Desktop, Cursor, or local Devin sessions

## Available Capabilities

### Tools (16)

**Discovery:**
- `list_nodes` — all node definitions (kind, schema, pins, categories)
- `get_node_definition(kind)` — single node lookup by kind
- `list_plugins` — installed plugins with version/type
- `list_samples(mode?)` — sample pipelines (filter: oneshot/dynamic/demo/user)
- `get_server_info` — server version, features, limits, plugin count

**Validation:**
- `validate_pipeline(yaml, mode?)` — validate YAML, get diagnostics
- `generate_oneshot_command(yaml, format)` — generate curl/skit-cli command

**Session lifecycle:**
- `create_session(yaml)` — create dynamic session from YAML
- `list_sessions` — list active sessions
- `get_pipeline(session_id)` — full pipeline state
- `destroy_session(session_id)` — stop and remove session

**Live mutation:**
- `validate_batch(session_id, operations)` — dry-run mutations
- `apply_batch(session_id, operations)` — apply mutations atomically
- `tune_node(session_id, node_id, message)` — send UpdateParams (requires `tune_nodes`)
- `update_pipeline(session_id, yaml)` — diff YAML against session, apply batch ops; auto-tunes params when caller has `tune_nodes` permission, otherwise returns `params_deferred`

**Diagnostics:**
- `get_logs(limit?, level?, filter?)` — recent server log lines

### Prompts (2)
- `design_pipeline(description?)` — node catalog + YAML format guide
- `debug_pipeline(session_id)` — pipeline state + diagnostic checklist

### Resources
- Sample pipelines as `streamkit://samples/{mode}/{id}` URIs
- `list_resources` / `read_resource` / `list_resource_templates`

## Common Workflows

### Design a new pipeline
```
1. Call prompt: design_pipeline (with optional description)
2. Call tool: validate_pipeline (check the YAML)
3. Call tool: create_session (deploy it)
```

### Browse and use sample pipelines
```
1. Call: list_resources (discover samples)
2. Call: read_resource (fetch YAML)
3. Customize the YAML
4. Call: validate_pipeline + create_session
```

### Debug a running session
```
1. Call: list_sessions (find the session)
2. Call prompt: debug_pipeline (get diagnostics)
3. Call: get_logs (check server logs)
4. Call: get_pipeline (inspect full state)
```

## Key Files
- `apps/skit/src/mcp/mod.rs` — tool implementations, resource handlers
- `apps/skit/src/mcp/prompts.rs` — prompt definitions
- `apps/skit/tests/mcp_integration_test.rs` — integration tests
- `agent_docs/mcp.md` — full reference documentation
