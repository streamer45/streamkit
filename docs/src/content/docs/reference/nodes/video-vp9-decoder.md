---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "video::vp9::decoder"
description: "Decodes VP9-compressed packets into raw NV12 video frames for CPU processing."
---

`kind`: `video::vp9::decoder`

Decodes VP9-compressed packets into raw NV12 video frames for CPU processing.

## Requirements
- Build with `--features video` and have `libvpx` available via `pkg-config`.

## Categories
- `video`
- `codecs`
- `vp9`

## Pins
### Inputs
- `in` accepts `EncodedVideo(EncodedVideoFormat { codec: Vp9 })` (one)

### Outputs
- `out` produces `RawVideo(RawVideoFormat { width: *, height: *, pixel_format: Nv12 })` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `threads` | `integer` | no | `2` | Decoder worker threads |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "threads": {
      "default": 2,
      "type": "integer"
    }
  },
  "title": "Vp9DecoderConfig",
  "type": "object"
}
```

</details>
