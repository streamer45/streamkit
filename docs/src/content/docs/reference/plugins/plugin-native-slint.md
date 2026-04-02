---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "plugin::native::slint"
description: "Slint UI rendering as a video source — render .slint files to RGBA8 frames at configurable resolution and frame rate."
---

`kind`: `plugin::native::slint` (original kind: `slint`)

Slint UI rendering as a video source — render `.slint` files to RGBA8 frames at configurable resolution and frame rate.

Source: `target/plugins/release/libslint_plugin.so`

## Categories
- `video`
- `generators`

## Pins
### Inputs
*(none — this is a source node)*

### Outputs
- `video` produces `RawVideo(RawVideoFormat { width: None, height: None, pixel_format: Rgba8 })` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `width` | `integer` | no | `640` | Output frame width in pixels<br />min: `1` |
| `height` | `integer` | no | `480` | Output frame height in pixels<br />min: `1` |
| `fps` | `integer` | no | `30` | Output frame rate<br />min: `1` |
| `slint_file` | `string` | yes | | Path to the `.slint` file |
| `component` | `string` | no | | Name of the exported component to instantiate (defaults to first) |
| `properties` | `object` | no | `{}` | Key-value map of Slint properties (strings, numbers, booleans) |
| `property_keyframes` | `array` | no | `[]` | List of property snapshots to cycle through over time |
| `keyframe_interval` | `integer` | no | `90` | Frames between keyframe switches<br />min: `1` |
| `frame_count` | `integer` | no | `0` | Total frames to generate (0 = infinite) |
| `static_ui` | `boolean` | no | `false` | Cache frames when properties haven't changed |

## Example Pipeline

```yaml
name: Slint Watermark
description: Renders a Slint UI overlay as a video source
mode: oneshot
steps:
  - kind: plugin::native::slint
    params:
      slint_file: "samples/slint/watermark.slint"
      width: 1280
      height: 720
      fps: 30
      static_ui: true
      properties:
        text: "StreamKit Live"
  - kind: video::vp9::encoder
    params:
      bitrate_kbps: 2000
  - kind: containers::webm::muxer
    params:
      streaming_mode: live
  - kind: streamkit::http_output
```


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "properties": {
    "width": {
      "default": 640,
      "description": "Output frame width in pixels",
      "minimum": 1,
      "type": "integer"
    },
    "height": {
      "default": 480,
      "description": "Output frame height in pixels",
      "minimum": 1,
      "type": "integer"
    },
    "fps": {
      "default": 30,
      "description": "Output frame rate",
      "minimum": 1,
      "type": "integer"
    },
    "slint_file": {
      "description": "Path to the .slint file",
      "type": "string"
    },
    "component": {
      "description": "Name of the exported component to instantiate (defaults to first)",
      "type": "string"
    },
    "properties": {
      "default": {},
      "description": "Key-value map of Slint properties (strings, numbers, booleans)",
      "type": "object"
    },
    "property_keyframes": {
      "default": [],
      "description": "List of property snapshots to cycle through over time",
      "items": {
        "type": "object"
      },
      "type": "array"
    },
    "keyframe_interval": {
      "default": 90,
      "description": "Frames between keyframe switches",
      "minimum": 1,
      "type": "integer"
    },
    "frame_count": {
      "default": 0,
      "description": "Total frames to generate (0 = infinite)",
      "type": "integer"
    },
    "static_ui": {
      "default": false,
      "description": "Cache frames when properties haven't changed",
      "type": "boolean"
    }
  },
  "required": [
    "slint_file"
  ],
  "type": "object"
}
```

</details>
