---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "video::vp9::decoder"
description: "Decodes VP9-compressed packets into raw NV12 video frames. Use this before CPU compositing or analysis pipelines."
---

`kind`: `video::vp9::decoder`

Decodes VP9-compressed packets into raw NV12 video frames. Use this before CPU compositing or analysis pipelines.

## Categories
- `video`
- `codecs`
- `vp9`

## Pins
### Inputs
- `in` accepts `EncodedVideo(EncodedVideoFormat { codec: Vp9, bitstream_format: None, codec_private: None, profile: None, level: None })` (one)

### Outputs
- `out` produces `RawVideo(RawVideoFormat { width: None, height: None, pixel_format: Nv12 })` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `threads` | `integer (uint32)` | no | `2` | min: `0` |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "threads": {
      "default": 2,
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    }
  },
  "title": "Vp9DecoderConfig",
  "type": "object"
}
```

</details>
