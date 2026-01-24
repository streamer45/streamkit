---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "Encoded Video"
description: "PacketType EncodedVideo structure"
---

`PacketType` id: `EncodedVideo`

Type system: `PacketType::EncodedVideo(EncodedVideoFormat)`

Runtime: `Packet::Binary { data, metadata, .. }`

## UI Metadata
- `label`: `Encoded Video`
- `color`: `#2980b9`
- `display_template`: `Encoded Video ({codec})`
- `compat: wildcard fields (bitstream_format, codec_private, profile, level), color: `#2980b9``

## Structure
Encoded video packets use `Packet::Binary`, with codec identity captured in the type system.

### PacketType payload (`EncodedVideoFormat`)

| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `codec` | `string enum[Vp9, H264, Av1]` | yes | — | Encoded video codec. |
| `bitstream_format` | `null | string enum[AnnexB, Avcc]` | no | — | Bitstream format hint (primarily for H264). Use `null` as a wildcard. |
| `codec_private` | `null | array<integer (uint8)>` | no | — | Optional codec-specific extradata. Use `null` as a wildcard. |
| `profile` | `null | string` | no | — | Optional codec profile hint. Use `null` as a wildcard. |
| `level` | `null | string` | no | — | Optional codec level hint. Use `null` as a wildcard. |

<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$defs": {
    "VideoBitstreamFormat": {
      "description": "Bitstream format hints for video codecs (primarily H264).",
      "enum": [
        "AnnexB",
        "Avcc"
      ],
      "type": "string"
    },
    "VideoCodec": {
      "description": "Supported encoded video codecs.",
      "enum": [
        "Vp9",
        "H264",
        "Av1"
      ],
      "type": "string"
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "description": "Encoded video format details (extensible for codec-specific config).",
  "properties": {
    "bitstream_format": {
      "anyOf": [
        {
          "$ref": "#/$defs/VideoBitstreamFormat"
        },
        {
          "type": "null"
        }
      ],
      "description": "Bitstream format hint (primarily for H264)."
    },
    "codec": {
      "$ref": "#/$defs/VideoCodec"
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
    },
    "level": {
      "description": "Optional codec level hint.",
      "type": [
        "string",
        "null"
      ]
    },
    "profile": {
      "description": "Optional codec profile hint.",
      "type": [
        "string",
        "null"
      ]
    }
  },
  "required": [
    "codec"
  ],
  "title": "EncodedVideoFormat",
  "type": "object"
}
```

</details>
