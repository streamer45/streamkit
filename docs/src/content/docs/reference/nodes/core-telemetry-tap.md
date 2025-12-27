---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "core::telemetry_tap"
description: "Observes packets and emits telemetry events for debugging and timeline visualization. Packets pass through unchanged while side-effect telemetry is sent to the session bus. Useful for monitoring Transcription, Custom (VAD), and other packet types."
---

`kind`: `core::telemetry_tap`

Observes packets and emits telemetry events for debugging and timeline visualization. Packets pass through unchanged while side-effect telemetry is sent to the session bus. Useful for monitoring Transcription, Custom (VAD), and other packet types.

## Categories
- `core`
- `observability`

## Pins
### Inputs
- `in` accepts `Any` (one)

### Outputs
- `out` produces `Passthrough` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `audio_sample_interval_ms` | `integer (uint64)` | no | `1000` | Audio sampling interval in milliseconds (for Audio packets).<br />Set to 0 to disable audio level events.<br />min: `0` |
| `event_type_filter` | `array<string>` | no | `[]` | Filter Custom packets by event_type pattern (glob-style).<br />Empty list means all Custom packets are included. |
| `max_events_per_sec` | `integer (uint32)` | no | `100` | Maximum events per second per event type.<br />min: `0` |
| `packet_types` | `array<string>` | no | `["Transcription","Custom"]` | Which packet types to convert to telemetry.<br />Default: `["Transcription", "Custom"]` |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "description": "Configuration for the telemetry tap node.",
  "properties": {
    "audio_sample_interval_ms": {
      "default": 1000,
      "description": "Audio sampling interval in milliseconds (for Audio packets).\nSet to 0 to disable audio level events.",
      "format": "uint64",
      "minimum": 0,
      "type": "integer"
    },
    "event_type_filter": {
      "default": [],
      "description": "Filter Custom packets by event_type pattern (glob-style).\nEmpty list means all Custom packets are included.",
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
  "title": "TelemetryTapConfig",
  "type": "object"
}
```

</details>
