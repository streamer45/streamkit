---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "Custom"
description: "PacketType Custom structure"
---

`PacketType` id: `Custom`

Type system: `PacketType::Custom { type_id }`

Runtime: `Packet::Custom(Arc<CustomPacketData>)`

## Why `Custom` exists
`Custom` is StreamKit's **extensibility escape hatch**: it lets plugins and pipelines exchange
structured data **without adding new built-in packet variants**.

It was designed to:

- Keep the core packet set small and stable (important for UIs and SDKs).
- Enable fast iteration for plugin-defined events and typed messages.
- Preserve type-checking via `type_id` so pipelines still validate before running.
- Stay user-friendly over JSON APIs (`encoding: "json"` is debuggable and easy to inspect).

## When to use it
Use `Custom` when you need **structured, typed messages** that don't fit existing packet types, for example:

- Plugin-defined events (e.g. VAD, moderation, scene triggers, rich status updates).
- Application-level envelopes (e.g. tool results, routing hints, structured logs).
- Telemetry-like events (see below) that you want to treat as first-class data.

Prefer other packet types when they fit:

- Audio frames/streams: `/reference/packets/raw-audio/` or `/reference/packets/opus-audio/`
- Plain strings: `/reference/packets/text/`
- Opaque bytes, blobs, or media: `/reference/packets/binary/`
- Speech-to-text results: `/reference/packets/transcription/`

## Type IDs, versioning, and compatibility
`type_id` is the **routing key** for `Custom` and is part of the type system.

- Compatibility: `PacketType::Custom { type_id: "a@1" }` only connects to the same `type_id`.
  If you truly want "any custom", use `PacketType::Any` on the input pin.
- Versioning: include a major version suffix like `@1` and bump it for breaking payload changes.
- Namespacing: use a stable, collision-resistant prefix (examples below).

Examples used in this repo:

- `core::telemetry/event@1` (telemetry envelope used on the WebSocket bus)
- `plugin::native::vad/vad-event@1` (VAD-style events)

## Payload conventions
`data` is schema-less JSON: treat it as **untrusted input** and validate it in consumers.

For "event"-shaped payloads, a common convention is an `event_type` string inside `data`:

```json
{
  "type_id": "core::telemetry/event@1",
  "encoding": "json",
  "data": { "event_type": "vad.start", "source": "mic" },
  "metadata": { "timestamp_us": 1735257600000000 }
}
```

Related docs:

- WebSocket telemetry events: `/reference/websocket-api/#telemetry-events-nodetelemetry`
- Nodes that observe/emit telemetry: `/reference/nodes/core-telemetry-tap/`, `/reference/nodes/core-telemetry-out/`

## When a core packet type is a better fit
`Custom` is great for iteration, but adding a new core packet type can be worth it when:

- The payload is **high-volume / performance-sensitive** (zero-copy, binary codecs, large frames).
- The payload needs **canonical semantics** across the ecosystem (multiple nodes, UIs, SDKs).
- There are **well-defined fields** that benefit from first-class schema/compat rules (not just `type_id`).
- The payload should be **universally inspectable/renderable** in the UI (timelines, previews, editors).

In those cases, open a GitHub issue describing the use case and examples (or send a PR). The goal is to keep
the built-in packet set small and stable, and graduate widely useful patterns out of `Custom` when needed.

## UI Metadata
- `label`: `Custom`
- `color`: `#e67e22`
- `display_template`: `Custom ({type_id})`
- `compat: wildcard fields (type_id), color: `#e67e22``

## Structure
Custom packets are carried as `Packet::Custom(Arc<CustomPacketData>)`.

| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `data` | `value` | yes | — | — |
| `encoding` | `string` | yes | — | Encoding for [`Packet::Custom`] payloads.<br /><br />This is intentionally extensible. For now we keep things user-friendly and debuggable. |
| `metadata` | `null | object` | no | — | Optional timing/ordering metadata. |
| `type_id` | `string` | yes | — | Namespaced, versioned type id (e.g., `plugin::native::vad/vad-event@1`). |

<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$defs": {
    "CustomEncoding": {
      "description": "Encoding for [`Packet::Custom`] payloads.\n\nThis is intentionally extensible. For now we keep things user-friendly and debuggable.",
      "oneOf": [
        {
          "const": "json",
          "description": "UTF-8 JSON value (object/array/string/number/bool/null).",
          "type": "string"
        }
      ]
    },
    "PacketMetadata": {
      "description": "Optional timing and sequencing metadata that can be attached to packets.\nUsed for pacing, synchronization, and A/V alignment.",
      "properties": {
        "duration_us": {
          "description": "Duration of this packet/frame in microseconds",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "sequence": {
          "description": "Sequence number for ordering and detecting loss",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "timestamp_us": {
          "description": "Absolute timestamp in microseconds (presentation time)",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        }
      },
      "type": "object"
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "description": "Extensible structured packet data.",
  "properties": {
    "data": true,
    "encoding": {
      "$ref": "#/$defs/CustomEncoding"
    },
    "metadata": {
      "anyOf": [
        {
          "$ref": "#/$defs/PacketMetadata"
        },
        {
          "type": "null"
        }
      ],
      "description": "Optional timing/ordering metadata."
    },
    "type_id": {
      "description": "Namespaced, versioned type id (e.g., `plugin::native::vad/vad-event@1`).",
      "type": "string"
    }
  },
  "required": [
    "type_id",
    "encoding",
    "data"
  ],
  "title": "CustomPacketData",
  "type": "object"
}
```

</details>
