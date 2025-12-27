---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "core::pacer"
description: "Controls packet flow rate by releasing packets at specified intervals. Useful for rate-limiting or simulating real-time data streams."
---

`kind`: `core::pacer`

Controls packet flow rate by releasing packets at specified intervals. Useful for rate-limiting or simulating real-time data streams.

## Categories
- `core`
- `timing`

## Pins
### Inputs
- `in` accepts `Any` (one)

### Outputs
- `out` produces `Passthrough` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `buffer_size` | `integer (uint)` | no | `16` | Maximum number of packets to buffer internally (for backpressure control)<br />Higher values = more memory, smoother pacing. Lower values = less memory, more backpressure.<br />Default: 16 packets (~320ms of audio at 20ms/frame)<br />min: `1` |
| `initial_burst_packets` | `integer (uint)` | no | `0` | Number of initial packets to send at 10x speed before starting paced delivery.<br />This builds up a client-side buffer to absorb network jitter.<br />Default: 0 (no initial burst). Recommended: 5-25 packets for networked streaming.<br />min: `0` |
| `speed` | `number (float)` | no | `1.0` | Playback speed multiplier (1.0 = real-time, 2.0 = 2x speed, 0.5 = half speed) |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "description": "Configuration for the PacerNode",
  "properties": {
    "buffer_size": {
      "default": 16,
      "description": "Maximum number of packets to buffer internally (for backpressure control)\nHigher values = more memory, smoother pacing. Lower values = less memory, more backpressure.\nDefault: 16 packets (~320ms of audio at 20ms/frame)",
      "format": "uint",
      "minimum": 1,
      "type": "integer"
    },
    "initial_burst_packets": {
      "default": 0,
      "description": "Number of initial packets to send at 10x speed before starting paced delivery.\nThis builds up a client-side buffer to absorb network jitter.\nDefault: 0 (no initial burst). Recommended: 5-25 packets for networked streaming.",
      "format": "uint",
      "minimum": 0,
      "type": "integer"
    },
    "speed": {
      "default": 1.0,
      "description": "Playback speed multiplier (1.0 = real-time, 2.0 = 2x speed, 0.5 = half speed)",
      "format": "float",
      "type": "number"
    }
  },
  "title": "PacerConfig",
  "type": "object"
}
```

</details>
