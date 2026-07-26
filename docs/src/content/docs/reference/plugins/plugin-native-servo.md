---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "plugin::native::servo"
description: "Renders a web page into RGBA8 video frames via the Servo browser engine. Navigates to the configured URL and produces frames at the specified resolution and frame rate."
---

`kind`: `plugin::native::servo` (original kind: `servo`)

Renders a web page into RGBA8 video frames via the Servo browser engine. Navigates to the configured URL and produces frames at the specified resolution and frame rate.

Source: `target/plugins/release/libservo_web.so`

## Categories
- `video`
- `generators`

## Pins
### Inputs
No inputs.

### Outputs

- `out` produces `RawVideo(Rgba8)` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `url` | `string` | yes | — | URL of the web page to render |
| `width` | `integer` | no | `1280` | Output frame width in pixels |
| `height` | `integer` | no | `720` | Output frame height in pixels |
| `viewport_width` | `integer` | no | `0` | Browser viewport width (`0` = output width) |
| `viewport_height` | `integer` | no | `0` | Browser viewport height (`0` = output height) |
| `viewport_resolution` | `string` | no | — | Runtime viewport preset (`640x480`, `1280x720`, `1280x960`, `1920x1080`, `2560x1440`) |
| `fps` | `integer` | no | `30` | Output frame rate |
| `custom_css` | `string` | no | — | Optional CSS to inject into the page |
| `frame_count` | `integer` | no | `0` | Total frames to generate (`0` = infinite) |
| `load_timeout_secs` | `integer` | no | `30` | Maximum seconds to hold frame emission for the initial page. Emission starts on load-complete, or ~2s after first paint when the load event lags |

<details>
<summary>Raw JSON Schema</summary>

```json
{
  "type": "object",
  "required": ["url"],
  "properties": {
    "url": { "type": "string", "description": "URL of the web page to render", "tunable": true },
    "width": { "type": "integer", "default": 1280, "minimum": 1 },
    "height": { "type": "integer", "default": 720, "minimum": 1 },
    "viewport_width": { "type": "integer", "default": 0 },
    "viewport_height": { "type": "integer", "default": 0 },
    "viewport_resolution": { "type": "string", "enum": ["640x480", "1280x720", "1280x960", "1920x1080", "2560x1440"], "tunable": true },
    "fps": { "type": "integer", "default": 30, "minimum": 1 },
    "custom_css": { "type": "string", "tunable": true },
    "frame_count": { "type": "integer", "default": 0 },
    "load_timeout_secs": { "type": "integer", "default": 30, "minimum": 1 }
  }
}
```
</details>
