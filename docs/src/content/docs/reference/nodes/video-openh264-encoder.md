---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "video::openh264::encoder"
description: "Encodes raw video frames (NV12 or I420) into H.264 Annex B packets using OpenH264 (Constrained Baseline profile). Insert a video::pixel_convert node upstream if the source outputs RGBA8."
---

`kind`: `video::openh264::encoder`

Encodes raw video frames (NV12 or I420) into H.264 Annex B packets using OpenH264 (Constrained Baseline profile). Insert a video::pixel_convert node upstream if the source outputs RGBA8.

## Categories
- `video`
- `codecs`
- `h264`

## Pins
### Inputs
- `in` accepts `RawVideo(RawVideoFormat { width: None, height: None, pixel_format: I420 }), RawVideo(RawVideoFormat { width: None, height: None, pixel_format: Nv12 })` (one)

### Outputs
- `out` produces `EncodedVideo(EncodedVideoFormat { codec: H264, bitstream_format: None, codec_private: None, profile: None, level: None })` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `bitrate_kbps` | `integer (uint32)` | no | `2000` | Target bitrate in kilobits per second.  Must be greater than zero.<br />min: `0` |
| `gop_size` | `integer (uint32)` | no | `60` | GOP size: number of frames between IDR (keyframe) insertions.<br /><br />0 = let the encoder decide (OpenH264 "auto" mode — may produce very<br />few keyframes).  For RTMP streaming to platforms like YouTube Live or<br />Twitch, set this to `2 × max_frame_rate` (e.g. 60 for 30fps) to get<br />a keyframe every 2 seconds, which is within the 2–4 s range most CDNs<br />require.<br /><br />Defaults to 60 (≈ 2 s at 30 fps).<br />min: `0` |
| `max_frame_rate` | `number (float)` | no | `30.0` | Maximum frame rate in Hz.  Must be greater than zero. |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Configuration for the OpenH264 encoder node.\n\nOpenH264 only supports Constrained Baseline profile (no B-frames, no\nCABAC).  This is well-suited for real-time / low-latency use cases.",
  "properties": {
    "bitrate_kbps": {
      "default": 2000,
      "description": "Target bitrate in kilobits per second.  Must be greater than zero.",
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    },
    "gop_size": {
      "default": 60,
      "description": "GOP size: number of frames between IDR (keyframe) insertions.\n\n0 = let the encoder decide (OpenH264 \"auto\" mode — may produce very\nfew keyframes).  For RTMP streaming to platforms like YouTube Live or\nTwitch, set this to `2 × max_frame_rate` (e.g. 60 for 30fps) to get\na keyframe every 2 seconds, which is within the 2–4 s range most CDNs\nrequire.\n\nDefaults to 60 (≈ 2 s at 30 fps).",
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    },
    "max_frame_rate": {
      "default": 30.0,
      "description": "Maximum frame rate in Hz.  Must be greater than zero.",
      "format": "float",
      "type": "number"
    }
  },
  "title": "OpenH264EncoderConfig",
  "type": "object"
}
```

</details>
