---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "audio::resampler"
description: "Converts audio between different sample rates using high-quality resampling. Essential for connecting nodes that operate at different sample rates."
---

`kind`: `audio::resampler`

Converts audio between different sample rates using high-quality resampling. Essential for connecting nodes that operate at different sample rates.

## Categories
- `audio`
- `filters`

## Pins
### Inputs
- `in` accepts `RawAudio(AudioFormat { sample_rate: 0, channels: 0, sample_format: F32 })` (one)

### Outputs
- `out` produces `RawAudio(AudioFormat { sample_rate: 48000, channels: 0, sample_format: F32 })` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `chunk_frames` | `integer (uint)` | no | `960` | Fixed chunk size for resampler (default: 960 frames = 20ms at 48kHz)<br />Larger values = better efficiency but more latency<br />min: `1` |
| `output_frame_size` | `integer (uint)` | no | `960` | Output frame size - packets will be buffered to this exact size (default: 960 = 20ms at 48kHz)<br />Must be a valid Opus frame size: 120, 240, 480, 960, 1920, or 2880 samples<br />Set to 0 to disable output buffering (variable frame sizes)<br />min: `0` |
| `target_sample_rate` | `integer (uint32)` | yes | — | Target output sample rate in Hz (e.g., 48000, 24000, 16000)<br />Input audio will be resampled to this rate<br />Must be greater than 0<br />min: `1` |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "description": "Configuration for the AudioResamplerNode",
  "properties": {
    "chunk_frames": {
      "default": 960,
      "description": "Fixed chunk size for resampler (default: 960 frames = 20ms at 48kHz)\nLarger values = better efficiency but more latency",
      "format": "uint",
      "minimum": 1,
      "type": "integer"
    },
    "output_frame_size": {
      "default": 960,
      "description": "Output frame size - packets will be buffered to this exact size (default: 960 = 20ms at 48kHz)\nMust be a valid Opus frame size: 120, 240, 480, 960, 1920, or 2880 samples\nSet to 0 to disable output buffering (variable frame sizes)",
      "format": "uint",
      "minimum": 0,
      "type": "integer"
    },
    "target_sample_rate": {
      "description": "Target output sample rate in Hz (e.g., 48000, 24000, 16000)\nInput audio will be resampled to this rate\nMust be greater than 0",
      "format": "uint32",
      "minimum": 1,
      "type": "integer"
    }
  },
  "required": [
    "target_sample_rate"
  ],
  "title": "AudioResamplerConfig",
  "type": "object"
}
```

</details>
