---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "plugin::native::piper"
description: "Text-to-speech synthesis using Piper TTS models. Lightweight and efficient for real-time applications. Supports multiple voices and languages. Outputs 22.05kHz mono audio."
---

`kind`: `plugin::native::piper` (original kind: `piper`)

Text-to-speech synthesis using Piper TTS models. Lightweight and efficient for real-time applications. Supports multiple voices and languages. Outputs 22.05kHz mono audio.

Source: `plugins/native/piper/target/release/libpiper.so`

## Categories
- `audio`
- `tts`

## Pins
### Inputs
- `in` accepts `Text` (one)

### Outputs
- `out` produces `RawAudio(AudioFormat { sample_rate: 22050, channels: 1, sample_format: F32 })` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `length_scale` | `number` | no | `1.0` | Duration control (0.5-2.0)<br />min: `0.5`<br />max: `2` |
| `min_sentence_length` | `integer` | no | `10` | Minimum chars before TTS generation<br />min: `1` |
| `model_dir` | `string` | yes | `./models/vits-piper-en_US-libritts_r-medium` | Path to Piper model directory |
| `noise_scale` | `number` | no | `0.667` | Voice variation control (0.0-1.0)<br />min: `0`<br />max: `1` |
| `noise_scale_w` | `number` | no | `0.8` | Prosody variation control (0.0-1.0)<br />min: `0`<br />max: `1` |
| `num_threads` | `integer` | no | `4` | CPU threads for inference<br />min: `1`<br />max: `16` |
| `speaker_id` | `integer` | no | `0` | Voice ID (model-dependent, 0-903 for libritts_r)<br />min: `0`<br />max: `903` |
| `speed` | `number` | no | `1.0` | Speech speed multiplier<br />min: `0.5`<br />max: `2` |

## Example Pipeline

```yaml
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
#
# SPDX-License-Identifier: MPL-2.0

name: Text-to-Speech (Piper)
description: Synthesizes speech from text using Piper
mode: oneshot
steps:
  - kind: streamkit::http_input
  - kind: core::text_chunker
    params:
      min_length: 10
  - kind: plugin::native::piper
    params:
      model_dir: "models/vits-piper-en_US-libritts_r-medium"
      speaker_id: 0
      speed: 1.0
      num_threads: 4
      noise_scale: 0.667
      noise_scale_w: 0.8
      length_scale: 1.0
  - kind: audio::resampler
    params:
      chunk_frames: 960
      output_frame_size: 960
      target_sample_rate: 48000
  - kind: audio::opus::encoder
  - kind: containers::webm::muxer
    params:
      channels: 1
      chunk_size: 65536
      sample_rate: 48000
      streaming_mode: live
  - kind: streamkit::http_output
```


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "properties": {
    "length_scale": {
      "default": 1.0,
      "description": "Duration control (0.5-2.0)",
      "maximum": 2.0,
      "minimum": 0.5,
      "type": "number"
    },
    "min_sentence_length": {
      "default": 10,
      "description": "Minimum chars before TTS generation",
      "minimum": 1,
      "type": "integer"
    },
    "model_dir": {
      "default": "./models/vits-piper-en_US-libritts_r-medium",
      "description": "Path to Piper model directory",
      "type": "string"
    },
    "noise_scale": {
      "default": 0.667,
      "description": "Voice variation control (0.0-1.0)",
      "maximum": 1.0,
      "minimum": 0.0,
      "type": "number"
    },
    "noise_scale_w": {
      "default": 0.8,
      "description": "Prosody variation control (0.0-1.0)",
      "maximum": 1.0,
      "minimum": 0.0,
      "type": "number"
    },
    "num_threads": {
      "default": 4,
      "description": "CPU threads for inference",
      "maximum": 16,
      "minimum": 1,
      "type": "integer"
    },
    "speaker_id": {
      "default": 0,
      "description": "Voice ID (model-dependent, 0-903 for libritts_r)",
      "maximum": 903,
      "minimum": 0,
      "type": "integer"
    },
    "speed": {
      "default": 1.0,
      "description": "Speech speed multiplier",
      "maximum": 2.0,
      "minimum": 0.5,
      "type": "number"
    }
  },
  "required": [
    "model_dir"
  ],
  "type": "object"
}
```

</details>
