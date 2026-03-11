---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "video::colorbars"
description: "Generates SMPTE EIA 75% color bar test frames. Supports NV12 (default), I420, and RGBA8 pixel formats via the pixel_format config. Use with a video encoder for pipeline testing and validation."
---

`kind`: `video::colorbars`

Generates SMPTE EIA 75% color bar test frames. Supports NV12 (default), I420, and RGBA8 pixel formats via the pixel_format config. Use with a video encoder for pipeline testing and validation.

## Categories
- `video`
- `generators`

## Pins
### Inputs
No inputs.

### Outputs
- `out` produces `RawVideo(RawVideoFormat { width: None, height: None, pixel_format: Nv12 })` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `animate` | `boolean` | no | `false` | When `true`, horizontally scrolls the color bars each frame so that<br />every frame differs substantially from the previous one.  Useful for<br />encoding benchmarks where static content would compress to nearly<br />nothing. |
| `draw_time` | `boolean` | no | `false` | When `true`, draws the current wall-clock time (`HH:MM:SS.mmm`)<br />onto each generated frame using a monospace font. |
| `draw_time_font_path` | `null | string` | no | `null` | Optional filesystem path to a custom TTF/OTF font used for the<br />`draw_time` overlay.  When omitted the bundled DejaVu Sans Mono<br />font (embedded in the binary) is used. |
| `fps` | `integer (uint32)` | no | `30` | Frames per second.<br />min: `0` |
| `frame_count` | `integer (uint32)` | no | `0` | Total frames to generate. 0 = infinite (real-time pacing).<br />min: `0` |
| `height` | `integer (uint32)` | no | `480` | Frame height in pixels.<br />min: `0` |
| `pixel_format` | `string` | no | `nv12` | Output pixel format. Supported: "nv12" (default), "i420", and "rgba8". |
| `width` | `integer (uint32)` | no | `640` | Frame width in pixels.<br />min: `0` |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "ColorBarsConfig",
  "description": "Configuration for the SMPTE color bars generator.",
  "type": "object",
  "properties": {
    "width": {
      "description": "Frame width in pixels.",
      "type": "integer",
      "format": "uint32",
      "minimum": 0,
      "default": 640
    },
    "height": {
      "description": "Frame height in pixels.",
      "type": "integer",
      "format": "uint32",
      "minimum": 0,
      "default": 480
    },
    "fps": {
      "description": "Frames per second.",
      "type": "integer",
      "format": "uint32",
      "minimum": 0,
      "default": 30
    },
    "frame_count": {
      "description": "Total frames to generate. 0 = infinite (real-time pacing).",
      "type": "integer",
      "format": "uint32",
      "minimum": 0,
      "default": 0
    },
    "pixel_format": {
      "description": "Output pixel format. Supported: \"nv12\" (default), \"i420\", and \"rgba8\".",
      "type": "string",
      "default": "nv12"
    },
    "draw_time": {
      "description": "When `true`, draws the current wall-clock time (`HH:MM:SS.mmm`)\nonto each generated frame using a monospace font.",
      "type": "boolean",
      "default": false
    },
    "draw_time_font_path": {
      "description": "Optional filesystem path to a custom TTF/OTF font used for the\n`draw_time` overlay.  When omitted the bundled DejaVu Sans Mono\nfont (embedded in the binary) is used.",
      "type": [
        "string",
        "null"
      ],
      "default": null
    },
    "animate": {
      "description": "When `true`, horizontally scrolls the color bars each frame so that\nevery frame differs substantially from the previous one.  Useful for\nencoding benchmarks where static content would compress to nearly\nnothing.",
      "type": "boolean",
      "default": false
    }
  }
}
```

</details>
