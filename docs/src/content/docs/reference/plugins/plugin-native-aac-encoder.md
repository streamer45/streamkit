---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "plugin::native::aac-encoder"
description: "AAC-LC audio encoder using FDK AAC (Fraunhofer). Accepts 48 kHz mono or stereo f32 PCM audio, outputs raw AAC frames. Mono input is automatically upmixed. Requires libfdk-aac.so.2 at runtime."
---

`kind`: `plugin::native::aac-encoder` (original kind: `aac-encoder`)

AAC-LC audio encoder using FDK AAC (Fraunhofer). Accepts 48 kHz mono or stereo f32 PCM audio, outputs raw AAC frames. Mono input is automatically upmixed. Requires libfdk-aac.so.2 at runtime.

Source: `target/plugins/release/libaac_encoder.so`

## Categories
- `audio`
- `codec`

## Pins
### Inputs
- `in` accepts `RawAudio(AudioFormat { sample_rate: 48000, channels: 2, sample_format: F32 })` or `RawAudio(AudioFormat { sample_rate: 48000, channels: 1, sample_format: F32 })` (one)

### Outputs

- `out` produces `EncodedAudio(Aac)` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `bitrate` | `integer` | no | `128000` | Target bitrate in bits per second; min `16000`, max `576000` |

<details>
<summary>Raw JSON Schema</summary>

```json
{
  "type": "object",
  "properties": {
    "bitrate": {
      "type": "integer",
      "description": "Target bitrate in bits per second",
      "default": 128000,
      "minimum": 16000,
      "maximum": 576000
    }
  }
}
```
</details>
