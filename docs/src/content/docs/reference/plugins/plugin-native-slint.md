---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "plugin::native::slint"
description: "Slint UI rendering as a video source — render .slint files to RGBA8 frames at configurable resolution and frame rate."
---

`kind`: `plugin::native::slint` (original kind: `slint`)

Slint UI rendering as a video source — render `.slint` files to RGBA8 frames at configurable resolution and frame rate.

Source: `target/plugins/release/libslint.so`

## Categories
- `video`
- `generators`

## Pins
### Inputs
*(none — this is a source node)*

### Outputs
- `out` produces `RawVideo(RawVideoFormat { width: None, height: None, pixel_format: Rgba8 })` (broadcast)

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

Composites a Slint watermark overlay onto colorbars and streams as WebM:

```yaml
name: Video Slint Watermark (Oneshot)
description: Composites colorbars with a Slint watermark overlay
mode: oneshot
client:
  input:
    type: none
  output:
    type: video

nodes:
  colorbars_bg:
    kind: video::colorbars
    params:
      width: 1280
      height: 720
      fps: 30
      frame_count: 300
      pixel_format: rgba8

  watermark:
    kind: plugin::native::slint
    params:
      width: 180
      height: 44
      fps: 30
      frame_count: 300
      slint_file: samples/slint/watermark.slint
      static_ui: true
      properties:
        channel: "StreamKit"
        tagline: "LIVE"

  compositor:
    kind: video::compositor
    params:
      width: 1280
      height: 720
      num_inputs: 2
      layers:
        in_0:
          opacity: 1.0
          z_index: 0
        in_1:
          rect:
            x: 1080
            y: 20
            width: 180
            height: 44
          opacity: 0.9
          z_index: 10
    needs:
      - colorbars_bg
      - watermark

  pixel_convert:
    kind: video::pixel_convert
    params:
      output_format: nv12
    needs: compositor

  vp9_encoder:
    kind: video::vp9::encoder
    needs: pixel_convert

  webm_muxer:
    kind: containers::webm::muxer
    params:
      video_width: 1280
      video_height: 720
      streaming_mode: live
    needs: vp9_encoder

  pacer:
    kind: core::pacer
    needs: webm_muxer

  http_output:
    kind: streamkit::http_output
    params:
      content_type: 'video/webm; codecs="vp9"'
    needs: pacer
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
