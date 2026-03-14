---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "Encoded Audio"
description: "PacketType EncodedAudio structure"
---

`PacketType` id: `EncodedAudio`

Type system: `PacketType::EncodedAudio(EncodedAudioFormat)`

Runtime: `Packet::Binary { data, metadata, .. }`

## UI Metadata
- `label`: `Encoded Audio`
- `color`: `#ff6b6b`
- `display_template`: `Encoded Audio ({codec})`
- compat: wildcard fields (`codec_private`), color: `#ff6b6b`

## Structure
Encoded audio packets use `Packet::Binary`, with codec identity captured in the type system.

### PacketType payload (`EncodedAudioFormat`)

| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `codec` | `string enum[Opus]` | yes | — | Encoded audio codec. |
| `codec_private` | `null | array<integer (uint8)>` | no | — | Optional codec-specific extradata. Use `null` as a wildcard. |

<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$defs": {
    "AudioCodec": {
      "description": "Supported encoded audio codecs.",
      "enum": [
        "Opus"
      ],
      "type": "string"
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "description": "Encoded audio format details (extensible for codec-specific config).",
  "properties": {
    "codec": {
      "$ref": "#/$defs/AudioCodec"
    },
    "codec_private": {
      "description": "Optional codec-specific extradata.",
      "items": {
        "format": "uint8",
        "maximum": 255,
        "minimum": 0,
        "type": "integer"
      },
      "type": [
        "array",
        "null"
      ]
    }
  },
  "required": [
    "codec"
  ],
  "title": "EncodedAudioFormat",
  "type": "object"
}
```

</details>
