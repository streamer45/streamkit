---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "audio::pacer"
description: "Controls audio playback timing by releasing frames at their natural rate. Useful for real-time streaming where audio should play at the correct speed rather than as fast as possible."
---

`kind`: `audio::pacer`

Controls audio playback timing by releasing frames at their natural rate. Useful for real-time streaming where audio should play at the correct speed rather than as fast as possible.

## Categories
- `audio`
- `timing`

## Pins
### Inputs
- `in` accepts `RawAudio(AudioFormat { sample_rate: 0, channels: 0, sample_format: F32 })` (one)

### Outputs
- `out` produces `Passthrough` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `buffer_size` | `integer (uint)` | no | `32` | Maximum number of audio frames to buffer internally<br />Default: 32 frames (~640ms of audio at 20ms/frame)<br />min: `1` |
| `generate_silence` | `boolean` | no | `true` | Generate silence frames when input queue is empty to maintain continuous stream<br />Prevents gaps in audio output (useful for real-time streaming protocols like MoQ)<br />Default: true |
| `initial_channels` | `integer | null (uint16)` | no | `null` | min: `0`<br />max: `65535` |
| `initial_sample_rate` | `integer | null (uint32)` | no | `null` | Optional initial audio format used to start pacing immediately (before the first input frame).<br /><br />Without an initial format, the pacer learns `(sample_rate, channels)` from the first<br />received frame. For pipelines that may take seconds before producing the first frame<br />(e.g., STT → LLM → TTS), this can cause downstream consumers to see a long gap and<br />underflow. Setting these lets the pacer emit silence right away.<br />min: `0` |
| `speed` | `number (float)` | no | `1.0` | Playback speed multiplier (1.0 = real-time, 2.0 = 2x speed, 0.5 = half speed) |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "description": "Configuration for the AudioPacerNode",
  "properties": {
    "buffer_size": {
      "default": 32,
      "description": "Maximum number of audio frames to buffer internally\nDefault: 32 frames (~640ms of audio at 20ms/frame)",
      "format": "uint",
      "minimum": 1,
      "type": "integer"
    },
    "generate_silence": {
      "default": true,
      "description": "Generate silence frames when input queue is empty to maintain continuous stream\nPrevents gaps in audio output (useful for real-time streaming protocols like MoQ)\nDefault: true",
      "type": "boolean"
    },
    "initial_channels": {
      "default": null,
      "format": "uint16",
      "maximum": 65535,
      "minimum": 0,
      "type": [
        "integer",
        "null"
      ]
    },
    "initial_sample_rate": {
      "default": null,
      "description": "Optional initial audio format used to start pacing immediately (before the first input frame).\n\nWithout an initial format, the pacer learns `(sample_rate, channels)` from the first\nreceived frame. For pipelines that may take seconds before producing the first frame\n(e.g., STT → LLM → TTS), this can cause downstream consumers to see a long gap and\nunderflow. Setting these lets the pacer emit silence right away.",
      "format": "uint32",
      "minimum": 0,
      "type": [
        "integer",
        "null"
      ]
    },
    "speed": {
      "default": 1.0,
      "description": "Playback speed multiplier (1.0 = real-time, 2.0 = 2x speed, 0.5 = half speed)",
      "format": "float",
      "type": "number"
    }
  },
  "title": "AudioPacerConfig",
  "type": "object"
}
```

</details>
