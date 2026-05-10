---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "video::av1::decoder"
description: "Decodes AV1-compressed packets into raw NV12 video frames using rav1d (pure-Rust dav1d). Use this before CPU compositing or analysis pipelines."
---

`kind`: `video::av1::decoder`

Decodes AV1-compressed packets into raw NV12 video frames using rav1d (pure-Rust dav1d). Use this before CPU compositing or analysis pipelines.

## Categories
- `video`
- `codecs`
- `av1`

## Pins
### Inputs
- `in` accepts `EncodedVideo(EncodedVideoFormat { codec: Av1, bitstream_format: None, codec_private: None, profile: None, level: None })` (one)

### Outputs
- `out` produces `RawVideo(RawVideoFormat { width: None, height: None, pixel_format: Nv12 })` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `threads` | `integer (uint32)` | no | `0` | Number of decoder threads.  `0` = auto-detect (rav1d picks a<br />thread count based on the number of logical cores, matching<br />C dav1d behaviour).<br />min: `0` |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "threads": {
      "default": 0,
      "description": "Number of decoder threads.  `0` = auto-detect (rav1d picks a\nthread count based on the number of logical cores, matching\nC dav1d behaviour).",
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    }
  },
  "title": "Av1DecoderConfig",
  "type": "object"
}
```

</details>
