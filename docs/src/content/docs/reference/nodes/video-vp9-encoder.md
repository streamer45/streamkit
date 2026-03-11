---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "video::vp9::encoder"
description: "Encodes raw video frames (NV12 or I420) into VP9 packets for transport or container muxing. Insert a video::pixel_convert node upstream if the source outputs RGBA8."
---

`kind`: `video::vp9::encoder`

Encodes raw video frames (NV12 or I420) into VP9 packets for transport or container muxing. Insert a video::pixel_convert node upstream if the source outputs RGBA8.

## Categories
- `video`
- `codecs`
- `vp9`

## Pins
### Inputs
- `in` accepts `RawVideo(RawVideoFormat { width: None, height: None, pixel_format: I420 }), RawVideo(RawVideoFormat { width: None, height: None, pixel_format: Nv12 })` (one)

### Outputs
- `out` produces `EncodedVideo(EncodedVideoFormat { codec: Vp9, bitstream_format: None, codec_private: None, profile: None, level: None })` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `bitrate_kbps` | `integer (uint32)` | no | `2500` | min: `0` |
| `cpu_used` | `integer (int32)` | no | `6` | libvpx `VP8E_SET_CPUUSED` control value.  Higher values trade quality<br />for speed.  Valid range depends on [`deadline`](Vp9EncoderDeadline):<br />  - `realtime`: 0–9 (default 6)<br />  - `good_quality` / `best_quality`: 0–5 |
| `deadline` | `string` | no | — | Controls the CPU time the VP9 encoder is allowed to spend per frame.<br /><br />Maps to the libvpx `deadline` parameter in `vpx_codec_encode`. |
| `keyframe_interval` | `integer (uint32)` | no | `120` | min: `0` |
| `threads` | `integer (uint32)` | no | `2` | min: `0` |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "Vp9EncoderConfig",
  "type": "object",
  "properties": {
    "bitrate_kbps": {
      "type": "integer",
      "format": "uint32",
      "minimum": 0,
      "default": 2500
    },
    "keyframe_interval": {
      "type": "integer",
      "format": "uint32",
      "minimum": 0,
      "default": 120
    },
    "threads": {
      "type": "integer",
      "format": "uint32",
      "minimum": 0,
      "default": 2
    },
    "deadline": {
      "$ref": "#/$defs/Vp9EncoderDeadline"
    },
    "cpu_used": {
      "description": "libvpx `VP8E_SET_CPUUSED` control value.  Higher values trade quality\nfor speed.  Valid range depends on [`deadline`](Vp9EncoderDeadline):\n  - `realtime`: 0–9 (default 6)\n  - `good_quality` / `best_quality`: 0–5",
      "type": "integer",
      "format": "int32",
      "default": 6
    }
  },
  "$defs": {
    "Vp9EncoderDeadline": {
      "description": "Controls the CPU time the VP9 encoder is allowed to spend per frame.\n\nMaps to the libvpx `deadline` parameter in `vpx_codec_encode`.",
      "oneOf": [
        {
          "description": "Real-time encoding – lowest latency, may sacrifice quality (VPX_DL_REALTIME).",
          "type": "string",
          "const": "realtime"
        },
        {
          "description": "Good quality – allows up to ~1 second per frame (VPX_DL_GOOD_QUALITY).",
          "type": "string",
          "const": "good_quality"
        },
        {
          "description": "Best quality – unlimited time per frame (VPX_DL_BEST_QUALITY).",
          "type": "string",
          "const": "best_quality"
        }
      ]
    }
  }
}
```

</details>
