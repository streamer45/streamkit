---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "transport::moq::subscriber"
description: "Subscribes to a Media over QUIC (MoQ) broadcast. Receives encoded Opus audio and VP9 video from a remote publisher over WebTransport."
---

`kind`: `transport::moq::subscriber`

Subscribes to a Media over QUIC (MoQ) broadcast. Receives encoded Opus audio and VP9 video from a remote publisher over WebTransport.

## Categories
- `transport`
- `moq`
- `dynamic`

## Pins
### Inputs
No inputs.

### Outputs
- `out` produces `EncodedAudio(EncodedAudioFormat { codec: Opus, codec_private: None })` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `broadcast` | `string` | no | — | — |
| `jwt` | `null | string` | no | `null` | Optional JWT for authenticated MoQ relays. When set, it is appended as `?jwt=...`.<br /><br />This is compatible with moq-relay and StreamKit's built-in MoQ auth. |
| `url` | `string` | no | — | — |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "MoqPullConfig",
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
    }
  }
}
```

</details>
