---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "transport::moq::subscriber"
description: "Subscribes to a Media over QUIC (MoQ) broadcast. Receives Opus audio from a remote publisher over WebTransport."
---

`kind`: `transport::moq::subscriber`

Subscribes to a Media over QUIC (MoQ) broadcast. Receives Opus audio from a remote publisher over WebTransport.

## Categories
- `transport`
- `moq`
- `dynamic`

## Pins
### Inputs
No inputs.

### Outputs
- `out` produces `OpusAudio` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `batch_ms` | `integer (uint64)` | no | `0` | Batch window in milliseconds. If > 0, after receiving a frame the node will<br />wait up to this duration to collect additional frames before forwarding.<br />Default: 0 (no batching) - recommended because moq_lite's TrackConsumer::read()<br />has internal allocation overhead that makes batching counterproductive.<br />min: `0` |
| `broadcast` | `string` | no | — | — |
| `url` | `string` | no | — | — |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "batch_ms": {
      "default": 0,
      "description": "Batch window in milliseconds. If > 0, after receiving a frame the node will\nwait up to this duration to collect additional frames before forwarding.\nDefault: 0 (no batching) - recommended because moq_lite's TrackConsumer::read()\nhas internal allocation overhead that makes batching counterproductive.",
      "format": "uint64",
      "minimum": 0,
      "type": "integer"
    },
    "broadcast": {
      "default": "",
      "type": "string"
    },
    "url": {
      "default": "",
      "type": "string"
    }
  },
  "title": "MoqPullConfig",
  "type": "object"
}
```

</details>
