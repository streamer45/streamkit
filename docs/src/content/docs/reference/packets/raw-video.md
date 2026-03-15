---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "Raw Video"
description: "PacketType RawVideo structure"
---

`PacketType` id: `RawVideo`

Type system: `PacketType::RawVideo(RawVideoFormat)`

Runtime: `Packet::Video(VideoFrame)`

## UI Metadata
- `label`: `Raw Video`
- `color`: `#1abc9c`
- `display_template`: `Raw Video ({width|*}x{height|*}, {pixel_format})`
- `compat: wildcard fields (width, height, pixel_format), color: `#1abc9c``

## Structure
Raw video is defined by a `RawVideoFormat` in the type system and carried as `Packet::Video(VideoFrame)` at runtime.

### PacketType payload (`RawVideoFormat`)

| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `height` | `integer | null (uint32)` | no | — | min: `0` |
| `pixel_format` | `string enum[Rgba8, I420, Nv12]` | yes | — | Describes the pixel format of raw video frames. |
| `width` | `integer | null (uint32)` | no | — | min: `0` |

<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "RawVideoFormat",
  "description": "Contains the detailed metadata for a raw video stream.",
  "type": "object",
  "properties": {
    "width": {
      "type": [
        "integer",
        "null"
      ],
      "format": "uint32",
      "minimum": 0
    },
    "height": {
      "type": [
        "integer",
        "null"
      ],
      "format": "uint32",
      "minimum": 0
    },
    "pixel_format": {
      "$ref": "#/$defs/PixelFormat"
    }
  },
  "required": [
    "pixel_format"
  ],
  "$defs": {
    "PixelFormat": {
      "description": "Describes the pixel format of raw video frames.",
      "type": "string",
      "enum": [
        "Rgba8",
        "I420",
        "Nv12"
      ]
    }
  }
}
```

</details>

### Runtime payload (`VideoFrame`)

`VideoFrame` is optimized for zero-copy fan-out (Arc + CoW). It contains:

- `layout` (`VideoLayout`) — plane offsets, strides, and dimensions
- `data` (shared byte buffer backed by `VideoFramePool`)
- `metadata` (`PacketMetadata`, optional)
