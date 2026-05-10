---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "video::av1::encoder"
description: "Encodes raw video frames (NV12 or I420) into AV1 packets using rav1e (pure-Rust). Insert a video::pixel_convert node upstream if the source outputs RGBA8."
---

`kind`: `video::av1::encoder`

Encodes raw video frames (NV12 or I420) into AV1 packets using rav1e (pure-Rust). Insert a video::pixel_convert node upstream if the source outputs RGBA8.

## Categories
- `video`
- `codecs`
- `av1`

## Pins
### Inputs
- `in` accepts `RawVideo(RawVideoFormat { width: None, height: None, pixel_format: I420 }), RawVideo(RawVideoFormat { width: None, height: None, pixel_format: Nv12 })` (one)

### Outputs
- `out` produces `EncodedVideo(EncodedVideoFormat { codec: Av1, bitstream_format: None, codec_private: None, profile: None, level: None })` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `bitrate_kbps` | `integer (uint32)` | no | `0` | min: `0` |
| `keyframe_interval` | `integer (uint32)` | no | `120` | min: `0` |
| `low_latency` | `boolean` | no | `true` | Enable rav1e low-latency mode (disables frame reordering). |
| `quantizer` | `integer (uint32)` | no | `80` | Constant-quality quantizer (0–255 scale, lower = better quality).<br /><br />Only used when `bitrate_kbps` is 0 (constant-quality mode).<br />Default: 80.<br />min: `0` |
| `speed` | `integer (uint32)` | no | `10` | rav1e speed preset (0 = slowest/best quality, 10 = fastest/real-time).<br />min: `0` |
| `threads` | `integer (uint32)` | no | `0` | Number of encoder threads.  `0` = auto-detect (rav1e delegates<br />to rayon, using all available logical cores).<br />min: `0` |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "bitrate_kbps": {
      "default": 0,
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    },
    "keyframe_interval": {
      "default": 120,
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    },
    "low_latency": {
      "default": true,
      "description": "Enable rav1e low-latency mode (disables frame reordering).",
      "type": "boolean"
    },
    "quantizer": {
      "default": 80,
      "description": "Constant-quality quantizer (0–255 scale, lower = better quality).\n\nOnly used when `bitrate_kbps` is 0 (constant-quality mode).\nDefault: 80.",
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    },
    "speed": {
      "default": 10,
      "description": "rav1e speed preset (0 = slowest/best quality, 10 = fastest/real-time).",
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    },
    "threads": {
      "default": 0,
      "description": "Number of encoder threads.  `0` = auto-detect (rav1e delegates\nto rayon, using all available logical cores).",
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    }
  },
  "title": "Av1EncoderConfig",
  "type": "object"
}
```

</details>
