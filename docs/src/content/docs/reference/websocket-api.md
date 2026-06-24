---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: WebSocket API
description: Control and events over /api/v1/control
---

StreamKit exposes a WebSocket control surface at `GET /api/v1/control` (WebSocket upgrade).

## Message Envelope

All messages are JSON objects:

```json
{
  "type": "request",
  "correlation_id": "abc123",
  "payload": { "action": "listnodes" }
}
```

- `type`: `request` | `response` | `event`
- `correlation_id`: recommended for `request` and echoed in the matching `response`
- `payload`: request/response payload tagged by `action`

## Action Names

Request and response payloads use `#[serde(rename_all = "lowercase")]` in the API contract.
That means action values are lowercased *without underscores*:

- `ListNodes` → `listnodes`
- `GetPipeline` → `getpipeline`
- `TuneNodeAsync` → `tunenodeasync`

If you're using TypeScript, generate bindings with `just gen-types` (outputs `ui/src/types/generated/api-types.ts`).

## Requests

Requests are sent as:

```json
{
  "type": "request",
  "correlation_id": "abc123",
  "payload": { "action": "getpipeline", "session_id": "sess_123" }
}
```

Supported `action` values:

- `createsession` `{ "name"?: string | null }`
- `destroysession` `{ "session_id": string }`
- `listsessions` `{}`
- `listnodes` `{}`
- `getpipeline` `{ "session_id": string }`
- `addnode` `{ "session_id": string, "node_id": string, "kind": string, "params"?: JsonValue | null }`
- `removenode` `{ "session_id": string, "node_id": string }`
- `connect` `{ "session_id": string, "from_node": string, "from_pin": string, "to_node": string, "to_pin": string, "mode"?: "reliable" | "best_effort" }`
- `disconnect` `{ "session_id": string, "from_node": string, "from_pin": string, "to_node": string, "to_pin": string }`
- `tunenode` `{ "session_id": string, "node_id": string, "message": NodeControlMessage }`
- `tunenodeasync` `{ "session_id": string, "node_id": string, "message": NodeControlMessage }`
- `validatebatch` `{ "session_id": string, "operations": BatchOperation[] }`
- `applybatch` `{ "session_id": string, "operations": BatchOperation[] }`
- `getpermissions` `{}`

If `mode` is omitted, it defaults to `reliable`.

**Re-adding a removed node id:** `removenode` is asynchronous — the engine
tears the node down in the background and the id stays in the pipeline until
a `noderemoved` event is broadcast. A separate `addnode` reusing the same id
before that event is rejected with "already exists in the pipeline". To
replace a node, either wait for its `noderemoved` event before re-adding, or
do the whole replacement in a single `applybatch` (`disconnect → removenode →
addnode → connect`), which resolves the id within the batch regardless of
teardown timing.

### Batch Operations

Batch operations allow multiple graph modifications to be validated or applied atomically.

**`BatchOperation` types:**

```json
// Add a node
{ "op": "addnode", "node_id": "gain1", "kind": "audio::gain", "params": { "gain": 1.0 } }

// Remove a node
{ "op": "removenode", "node_id": "gain1" }

// Connect two nodes
{ "op": "connect", "from_node": "input", "from_pin": "out", "to_node": "gain1", "to_pin": "in", "mode": "reliable" }

// Disconnect two nodes
{ "op": "disconnect", "from_node": "input", "from_pin": "out", "to_node": "gain1", "to_pin": "in" }
```

**Validation vs Apply:**
- `validatebatch`: Checks if operations are valid without applying them. Returns `validationresult` with success/failure.
- `applybatch`: Applies operations atomically. All succeed or all fail together. Returns `batchapplied` on success.

## Responses

Responses are sent as:

```json
{
  "type": "response",
  "correlation_id": "abc123",
  "payload": { "action": "pipeline", "pipeline": { "mode": "dynamic", "nodes": {}, "connections": [] } }
}
```

Response `action` values include:

- `sessioncreated`, `sessiondestroyed`
- `sessionslist`, `nodeslist`, `pipeline`
- `validationresult`, `batchapplied`
- `permissions`, `success`, `error`

## Events

Events are broadcast to all connected clients. All event payloads are tagged by `event` (not `action`).

| Event | Description |
|-------|-------------|
| `nodestatechanged` | A node's state changed (e.g. `Running`, `Ready`, `Recovering`) |
| `nodestatsupdated` | Throttled stats update (packets processed / discarded / errored) |
| `nodeparamschanged` | A node's parameters were updated at runtime |
| `sessioncreated` | A new session was created |
| `sessiondestroyed` | A session was destroyed |
| `nodeadded` | A node was added to a session graph |
| `noderemoved` | A node was removed from a session graph |
| `connectionadded` | An edge was added between two nodes |
| `connectionremoved` | An edge was removed between two nodes |
| `nodeviewdataupdated` | Node-specific view data changed (e.g. compositor resolved layout) |
| `nodetelemetry` | Out-of-band telemetry from a node (see below) |
| `runtimeschemasupdated` | A node's runtime param schema was discovered after init |

Example:

```json
{
  "type": "event",
  "payload": {
    "event": "nodestatechanged",
    "session_id": "sess_123",
    "node_id": "node_1",
    "state": "Running",
    "timestamp": "2025-01-01T00:00:00Z"
  }
}
```

Notes:
- For simple states, `state` is a string (e.g. `"Running"`, `"Ready"`).
- For states with additional data, `state` is an object keyed by the state name (e.g. `{ "Recovering": { "reason": "...", "details": null } }`).

### Telemetry events (`nodetelemetry`)

Some nodes can emit out-of-band telemetry events to a per-session telemetry bus. These are delivered over the same WebSocket as `nodetelemetry` events:

```json
{
  "type": "event",
  "payload": {
    "event": "nodetelemetry",
    "session_id": "sess_123",
    "node_id": "whisper_stt",
    "type_id": "core::telemetry/event@1",
    "data": {
      "event_type": "stt.result",
      "text_preview": "hello world...",
      "segment_count": 3
    },
    "timestamp_us": 1733964469123456,
    "timestamp": "2025-01-01T00:00:00Z"
  }
}
```

Notes:
- Telemetry is **best-effort** and may be dropped under load.
- The server may truncate large string fields before forwarding (to keep the control plane responsive).

## Error Handling

Error responses have `action: "error"` with a message field:

```json
{
  "type": "response",
  "correlation_id": "abc123",
  "payload": { "action": "error", "message": "Session 'sess_123' not found" }
}
```

**Common error scenarios:**
- Session not found
- Permission denied
- Invalid node kind
- Invalid connection (incompatible pin types)
- Node not found in session

**Distinguishing success from error:** Check the `action` field in the response payload. Success responses have action-specific values (e.g., `sessioncreated`, `pipeline`), while errors always have `action: "error"`.

---

The authoritative payload shapes live in the `streamkit-api` crate and are used to generate TypeScript bindings for the UI.
