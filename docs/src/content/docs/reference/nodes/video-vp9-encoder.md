---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "video::vp9::encoder"
description: "Encodes raw I420 video frames into VP9 packets for transport or container muxing."
---

`kind`: `video::vp9::encoder`

Encodes raw I420 video frames into VP9 packets for transport or container muxing.

## Requirements
- Build with `--features video` and have `libvpx` available via `pkg-config`.

## Categories
- `video`
- `codecs`
- `vp9`

## Pins
### Inputs
- `in` accepts `RawVideo(RawVideoFormat { width: *, height: *, pixel_format: I420 })` (one)

### Outputs
- `out` produces `EncodedVideo(EncodedVideoFormat { codec: Vp9 })` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `bitrate_kbps` | `integer` | no | `2500` | Target bitrate in kbps |
| `keyframe_interval` | `integer` | no | `120` | Maximum keyframe interval (frames) |
| `threads` | `integer` | no | `2` | Encoder worker threads |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "bitrate_kbps": {
      "default": 2500,
      "type": "integer"
    },
    "keyframe_interval": {
      "default": 120,
      "type": "integer"
    },
    "threads": {
      "default": 2,
      "type": "integer"
    }
  },
  "title": "Vp9EncoderConfig",
  "type": "object"
}
```

</details>
