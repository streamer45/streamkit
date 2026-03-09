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
- `compat: wildcard fields (width, height), color: `#1abc9c``

## Structure
Raw video is defined by a `RawVideoFormat` in the type system and carried as `Packet::Video(VideoFrame)` at runtime.

Use `null` for `width` or `height` when you want wildcard/unknown dimensions.

### PacketType payload (`RawVideoFormat`)

| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `width` | `null | integer (uint32)` | no | — | Frame width in pixels. `null` acts as a wildcard. |
| `height` | `null | integer (uint32)` | no | — | Frame height in pixels. `null` acts as a wildcard. |
| `pixel_format` | `string enum[Rgba8, I420, Nv12]` | yes | — | Pixel format for raw frames. |

<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$defs": {
    "PixelFormat": {
      "description": "Describes the pixel format of raw video frames.",
      "enum": [
        "Rgba8",
        "I420",
        "Nv12"
      ],
      "type": "string"
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "description": "Contains the detailed metadata for a raw video stream.",
  "properties": {
    "height": {
      "format": "uint32",
      "minimum": 0,
      "type": [
        "integer",
        "null"
      ]
    },
    "pixel_format": {
      "$ref": "#/$defs/PixelFormat"
    },
    "width": {
      "format": "uint32",
      "minimum": 0,
      "type": [
        "integer",
        "null"
      ]
    }
  },
  "required": [
    "pixel_format"
  ],
  "title": "RawVideoFormat",
  "type": "object"
}
```

</details>

### Runtime payload (`VideoFrame`)

`VideoFrame` is optimized for zero-copy fan-out. It contains:

- `width` (u32)
- `height` (u32)
- `pixel_format` (`PixelFormat`)
- `layout` (`VideoLayout`, includes per-plane offsets/strides and `stride_align`)
- `data` (packed bytes; layout depends on the pixel format)
- `metadata` (`PacketMetadata`, optional)

`VideoLayout` exposes:
- `plane_count`
- `planes[]` with `offset`, `stride`, `width`, `height`
- `total_bytes`
- `stride_align` (byte alignment used for each plane stride)

StreamKit assumes raw video frames use a canonical aligned layout (as produced by `VideoLayout::aligned`).
Codec nodes may reject frames whose layout does not match the expected canonical layout.
