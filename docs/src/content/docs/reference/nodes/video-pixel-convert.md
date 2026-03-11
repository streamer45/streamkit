---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "video::pixel_convert"
description: "Converts raw video frames between pixel formats (RGBA8, NV12, I420). Insert upstream of nodes that require a specific format (e.g. VP9 encoder). Passthrough when input format already matches the target."
---

`kind`: `video::pixel_convert`

Converts raw video frames between pixel formats (RGBA8, NV12, I420). Insert upstream of nodes that require a specific format (e.g. VP9 encoder). Passthrough when input format already matches the target.

## Categories
- `video`
- `convert`

## Pins
### Inputs
- `in` accepts `RawVideo(RawVideoFormat { width: None, height: None, pixel_format: Rgba8 }), RawVideo(RawVideoFormat { width: None, height: None, pixel_format: I420 }), RawVideo(RawVideoFormat { width: None, height: None, pixel_format: Nv12 })` (one)

### Outputs
- `out` produces `RawVideo(RawVideoFormat { width: None, height: None, pixel_format: Nv12 })` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `output_format` | `string` | no | `nv12` | Target pixel format: `"nv12"` (default), `"i420"`, or `"rgba8"`. |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "PixelConvertConfig",
  "description": "Configuration for the pixel format converter node.",
  "type": "object",
  "properties": {
    "output_format": {
      "description": "Target pixel format: `\"nv12\"` (default), `\"i420\"`, or `\"rgba8\"`.",
      "type": "string",
      "default": "nv12"
    }
  }
}
```

</details>
