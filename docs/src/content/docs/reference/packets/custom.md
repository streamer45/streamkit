---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "Custom"
description: "PacketType Custom structure"
---

`PacketType` id: `Custom`

Type system: `PacketType::Custom { type_id }`

Runtime: `Packet::Custom(Arc<CustomPacketData>)`

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
