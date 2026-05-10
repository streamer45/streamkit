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
  "$defs": {
    "Vp9EncoderDeadline": {
      "description": "Controls the CPU time the VP9 encoder is allowed to spend per frame.\n\nMaps to the libvpx `deadline` parameter in `vpx_codec_encode`.",
      "oneOf": [
        {
          "const": "realtime",
          "description": "Real-time encoding – lowest latency, may sacrifice quality (VPX_DL_REALTIME).",
          "type": "string"
        },
        {
          "const": "good_quality",
          "description": "Good quality – allows up to ~1 second per frame (VPX_DL_GOOD_QUALITY).",
          "type": "string"
        },
        {
          "const": "best_quality",
          "description": "Best quality – unlimited time per frame (VPX_DL_BEST_QUALITY).",
          "type": "string"
        }
      ]
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "bitrate_kbps": {
      "default": 2500,
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    },
    "cpu_used": {
      "default": 6,
      "description": "libvpx `VP8E_SET_CPUUSED` control value.  Higher values trade quality\nfor speed.  Valid range depends on [`deadline`](Vp9EncoderDeadline):\n  - `realtime`: 0–9 (default 6)\n  - `good_quality` / `best_quality`: 0–5",
      "format": "int32",
      "type": "integer"
    },
    "deadline": {
      "$ref": "#/$defs/Vp9EncoderDeadline"
    },
    "keyframe_interval": {
      "default": 120,
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    },
    "threads": {
      "default": 2,
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    }
  },
  "title": "Vp9EncoderConfig",
  "type": "object"
}
```

</details>
