---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "audio::gain"
description: "Adjusts audio volume by applying a linear gain multiplier to all samples. Supports real-time parameter tuning for live volume control."
---

`kind`: `audio::gain`

Adjusts audio volume by applying a linear gain multiplier to all samples. Supports real-time parameter tuning for live volume control.

## Categories
- `audio`
- `filters`

## Pins
### Inputs
- `in` accepts `RawAudio(AudioFormat { sample_rate: 0, channels: 0, sample_format: F32 })` (one)

### Outputs
- `out` produces `RawAudio(AudioFormat { sample_rate: 0, channels: 0, sample_format: F32 })` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `gain` | `number` | no | `1.0` | A linear multiplier for the audio amplitude (e.g., 0.5 is -6dB).<br />This parameter can be updated in real-time while the node is running.<br />Valid range: 0.0 to 4.0<br />min: `0`<br />max: `4` |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "description": "The configuration struct for the AudioGainNode.",
  "properties": {
    "gain": {
      "default": 1.0,
      "description": "A linear multiplier for the audio amplitude (e.g., 0.5 is -6dB).\nThis parameter can be updated in real-time while the node is running.\nValid range: 0.0 to 4.0",
      "maximum": 4.0,
      "minimum": 0.0,
      "tunable": true,
      "type": "number"
    }
  },
  "title": "AudioGainConfig",
  "type": "object"
}
```

</details>
