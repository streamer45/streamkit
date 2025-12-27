---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "transport::moq::publisher"
description: "Publishes audio to a Media over QUIC (MoQ) broadcast. Sends Opus audio to subscribers over WebTransport."
---

`kind`: `transport::moq::publisher`

Publishes audio to a Media over QUIC (MoQ) broadcast. Sends Opus audio to subscribers over WebTransport.

## Categories
- `transport`
- `moq`
- `dynamic`

## Pins
### Inputs
- `in` accepts `OpusAudio` (one)

### Outputs
No outputs.

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `broadcast` | `string` | no | — | — |
| `channels` | `integer (uint32)` | no | `2` | min: `0` |
| `group_duration_ms` | `integer (uint64)` | no | `40` | Duration of each MoQ group in milliseconds.<br />Smaller groups = lower latency but more overhead.<br />Larger groups = higher latency but better efficiency.<br />Default: 40ms (2 Opus frames at 20ms each).<br />For real-time applications, use 20-60ms. For high-latency networks, use 100ms+.<br />min: `0` |
| `initial_delay_ms` | `integer (uint64)` | no | `0` | Adds a timestamp offset (playout delay) so receivers can buffer before playback.<br /><br />This is especially helpful when subscribers are on higher-latency / higher-jitter links,<br />and the client begins playback as soon as it sees the first frame.<br /><br />Default: 0 (no added delay).<br />min: `0` |
| `url` | `string` | no | — | — |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "broadcast": {
      "default": "",
      "type": "string"
    },
    "channels": {
      "default": 2,
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    },
    "group_duration_ms": {
      "default": 40,
      "description": "Duration of each MoQ group in milliseconds.\nSmaller groups = lower latency but more overhead.\nLarger groups = higher latency but better efficiency.\nDefault: 40ms (2 Opus frames at 20ms each).\nFor real-time applications, use 20-60ms. For high-latency networks, use 100ms+.",
      "format": "uint64",
      "minimum": 0,
      "type": "integer"
    },
    "initial_delay_ms": {
      "default": 0,
      "description": "Adds a timestamp offset (playout delay) so receivers can buffer before playback.\n\nThis is especially helpful when subscribers are on higher-latency / higher-jitter links,\nand the client begins playback as soon as it sees the first frame.\n\nDefault: 0 (no added delay).",
      "format": "uint64",
      "minimum": 0,
      "type": "integer"
    },
    "url": {
      "default": "",
      "type": "string"
    }
  },
  "title": "MoqPushConfig",
  "type": "object"
}
```

</details>
