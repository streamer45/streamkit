---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "transport::moq::peer"
description: "Bidirectional MoQ peer for real-time audio communication. Acts as both publisher and subscriber over a single WebTransport connection."
---

`kind`: `transport::moq::peer`

Bidirectional MoQ peer for real-time audio communication. Acts as both publisher and subscriber over a single WebTransport connection.

## Categories
- `transport`
- `moq`
- `bidirectional`
- `dynamic`

## Pins
### Inputs
- `in` accepts `OpusAudio` (one)

### Outputs
- `out` produces `OpusAudio` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `allow_reconnect` | `boolean` | no | `false` | Allow publisher reconnections without recreating the session |
| `gateway_path` | `string` | no | `/moq` | Base path for gateway routing (e.g., "/moq")<br />Publishers connect to "{gateway_path}/input", subscribers to "{gateway_path}/output" |
| `input_broadcast` | `string` | no | `input` | Broadcast name to receive from publisher client |
| `output_broadcast` | `string` | no | `output` | Broadcast name to send to subscriber clients |
| `output_group_duration_ms` | `integer (uint64)` | no | `40` | Duration of each MoQ group in milliseconds for the subscriber output.<br /><br />Default: 40ms (2 Opus frames at 20ms each).<br />min: `0` |
| `output_initial_delay_ms` | `integer (uint64)` | no | `0` | Adds a timestamp offset (playout delay) so receivers can buffer before playback.<br /><br />Default: 0 (no added delay).<br />min: `0` |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "allow_reconnect": {
      "default": false,
      "description": "Allow publisher reconnections without recreating the session",
      "type": "boolean"
    },
    "gateway_path": {
      "default": "/moq",
      "description": "Base path for gateway routing (e.g., \"/moq\")\nPublishers connect to \"{gateway_path}/input\", subscribers to \"{gateway_path}/output\"",
      "type": "string"
    },
    "input_broadcast": {
      "default": "input",
      "description": "Broadcast name to receive from publisher client",
      "type": "string"
    },
    "output_broadcast": {
      "default": "output",
      "description": "Broadcast name to send to subscriber clients",
      "type": "string"
    },
    "output_group_duration_ms": {
      "default": 40,
      "description": "Duration of each MoQ group in milliseconds for the subscriber output.\n\nDefault: 40ms (2 Opus frames at 20ms each).",
      "format": "uint64",
      "minimum": 0,
      "type": "integer"
    },
    "output_initial_delay_ms": {
      "default": 0,
      "description": "Adds a timestamp offset (playout delay) so receivers can buffer before playback.\n\nDefault: 0 (no added delay).",
      "format": "uint64",
      "minimum": 0,
      "type": "integer"
    }
  },
  "title": "MoqPeerConfig",
  "type": "object"
}
```

</details>
