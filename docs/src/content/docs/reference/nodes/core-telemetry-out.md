---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "core::telemetry_out"
description: "Consumes packets and emits telemetry events to the session bus (WebSocket). This is a terminal node intended for best-effort side branches."
---

`kind`: `core::telemetry_out`

Consumes packets and emits telemetry events to the session bus (WebSocket). This is a terminal node intended for best-effort side branches.

## Categories
- `core`
- `observability`

## Pins
### Inputs
- `in` accepts `Any` (one)

### Outputs
No outputs.

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `event_type_filter` | `array<string>` | no | `[]` | Filter event types (glob-style prefix patterns like `vad.*`).<br />Empty list means all events are included. |
| `max_events_per_sec` | `integer (uint32)` | no | `100` | Maximum events per second per event type.<br />min: `0` |
| `packet_types` | `array<string>` | no | `["Transcription","Custom"]` | Which packet types to convert to telemetry.<br />Default: `["Transcription", "Custom"]` |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "event_type_filter": {
      "default": [],
      "description": "Filter event types (glob-style prefix patterns like `vad.*`).\nEmpty list means all events are included.",
      "items": {
        "type": "string"
      },
      "type": "array"
    },
    "max_events_per_sec": {
      "default": 100,
      "description": "Maximum events per second per event type.",
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    },
    "packet_types": {
      "default": [
        "Transcription",
        "Custom"
      ],
      "description": "Which packet types to convert to telemetry.\nDefault: `[\"Transcription\", \"Custom\"]`",
      "items": {
        "type": "string"
      },
      "type": "array"
    }
  },
  "title": "TelemetryOutConfig",
  "type": "object"
}
```

</details>
