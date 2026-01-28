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
| `jwt` | `null | string` | no | `null` | Optional JWT for authenticated MoQ relays. When set, it is appended as `?jwt=...`.<br /><br />This is compatible with moq-relay and StreamKit's built-in MoQ auth. |
| `url` | `string` | no | — | — |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "MoqPushConfig",
  "type": "object",
  "properties": {
    "url": {
      "type": "string",
      "default": ""
    },
    "jwt": {
      "description": "Optional JWT for authenticated MoQ relays. When set, it is appended as `?jwt=...`.\n\nThis is compatible with moq-relay and StreamKit's built-in MoQ auth.",
      "type": [
        "string",
        "null"
      ],
      "default": null
    },
    "broadcast": {
      "type": "string",
      "default": ""
    },
    "channels": {
      "type": "integer",
      "format": "uint32",
      "minimum": 0,
      "default": 2
    },
    "group_duration_ms": {
      "description": "Duration of each MoQ group in milliseconds.\nSmaller groups = lower latency but more overhead.\nLarger groups = higher latency but better efficiency.\nDefault: 40ms (2 Opus frames at 20ms each).\nFor real-time applications, use 20-60ms. For high-latency networks, use 100ms+.",
      "type": "integer",
      "format": "uint64",
      "minimum": 0,
      "default": 40
    },
    "initial_delay_ms": {
      "description": "Adds a timestamp offset (playout delay) so receivers can buffer before playback.\n\nThis is especially helpful when subscribers are on higher-latency / higher-jitter links,\nand the client begins playback as soon as it sees the first frame.\n\nDefault: 0 (no added delay).",
      "type": "integer",
      "format": "uint64",
      "minimum": 0,
      "default": 0
    }
  }
}
```

</details>
