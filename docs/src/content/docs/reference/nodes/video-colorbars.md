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
| `draw_time` | `boolean` | no | `false` | When `true`, draws the current wall-clock time (`HH:MM:SS.mmm`)<br />onto each generated frame using a monospace font.<br /><br />See also [`draw_time_use_pts`](Self::draw_time_use_pts) for an<br />alternative time source. |
| `draw_time_font_path` | `null | string` | no | `null` | Optional filesystem path to a custom TTF/OTF font used for the<br />`draw_time` overlay.  When omitted the bundled DejaVu Sans Mono<br />font (embedded in the binary) is used. |
| `draw_time_use_pts` | `boolean` | no | `false` | When `true` (and `draw_time` is enabled), stamps the frame's<br />presentation timestamp (PTS) instead of the wall-clock time.<br />This is more useful for debugging A/V timing since the stamped<br />value matches the metadata the downstream pipeline sees. |
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
  "additionalProperties": false,
  "description": "Configuration for the SMPTE color bars generator.",
  "properties": {
    "animate": {
      "default": false,
      "description": "When `true`, horizontally scrolls the color bars each frame so that\nevery frame differs substantially from the previous one.  Useful for\nencoding benchmarks where static content would compress to nearly\nnothing.",
      "type": "boolean"
    },
    "draw_time": {
      "default": false,
      "description": "When `true`, draws the current wall-clock time (`HH:MM:SS.mmm`)\nonto each generated frame using a monospace font.\n\nSee also [`draw_time_use_pts`](Self::draw_time_use_pts) for an\nalternative time source.",
      "type": "boolean"
    },
    "draw_time_font_path": {
      "default": null,
      "description": "Optional filesystem path to a custom TTF/OTF font used for the\n`draw_time` overlay.  When omitted the bundled DejaVu Sans Mono\nfont (embedded in the binary) is used.",
      "type": [
        "string",
        "null"
      ]
    },
    "draw_time_use_pts": {
      "default": false,
      "description": "When `true` (and `draw_time` is enabled), stamps the frame's\npresentation timestamp (PTS) instead of the wall-clock time.\nThis is more useful for debugging A/V timing since the stamped\nvalue matches the metadata the downstream pipeline sees.",
      "type": "boolean"
    },
    "fps": {
      "default": 30,
      "description": "Frames per second.",
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    },
    "frame_count": {
      "default": 0,
      "description": "Total frames to generate. 0 = infinite (real-time pacing).",
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    },
    "height": {
      "default": 480,
      "description": "Frame height in pixels.",
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    },
    "pixel_format": {
      "default": "nv12",
      "description": "Output pixel format. Supported: \"nv12\" (default), \"i420\", and \"rgba8\".",
      "type": "string"
    },
    "width": {
      "default": 640,
      "description": "Frame width in pixels.",
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    }
  },
  "title": "ColorBarsConfig",
  "type": "object"
}
```

</details>
