---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "plugin::native::matcha"
description: "Text-to-speech synthesis using Matcha-TTS, a fast non-autoregressive model. Provides high-quality speech with efficient inference. Outputs 22.05kHz mono audio."
---

`kind`: `plugin::native::matcha` (original kind: `matcha`)

Text-to-speech synthesis using Matcha-TTS, a fast non-autoregressive model. Provides high-quality speech with efficient inference. Outputs 22.05kHz mono audio.

Source: `plugins/native/matcha/target/release/libmatcha.so`

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
| `execution_provider` | `string enum[cpu, cuda, tensorrt]` | no | `cpu` | ONNX Runtime execution provider (requires libsherpa-onnx built with GPU support) |
| `length_scale` | `number` | no | `1.0` | Duration control (alternative to speed)<br />min: `0.5`<br />max: `2` |
| `min_sentence_length` | `integer` | no | `10` | Minimum chars before TTS generation<br />min: `1` |
| `model_dir` | `string` | yes | `./models/matcha-icefall-en_US-ljspeech` | Path to Matcha model directory |
| `noise_scale` | `number` | no | `0.667` | Voice variation control<br />min: `0`<br />max: `1` |
| `num_threads` | `integer` | no | `4` | CPU threads for inference<br />min: `1`<br />max: `16` |
| `speaker_id` | `integer` | no | `0` | Voice ID (0 for LJSpeech single-speaker model)<br />min: `0` |
| `speed` | `number` | no | `1.0` | Speech speed multiplier<br />min: `0.5`<br />max: `2` |

## Example Pipeline

```yaml
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
#
# SPDX-License-Identifier: MPL-2.0

name: Text-to-Speech (Matcha)
description: Synthesizes speech from text using Matcha
mode: oneshot
steps:
  - kind: streamkit::http_input
  - kind: core::text_chunker
    params:
      min_length: 10
  - kind: plugin::native::matcha
    params:
      model_dir: "models/matcha-icefall-en_US-ljspeech"
      speaker_id: 0
      speed: 1.0
      noise_scale: 0.667
      length_scale: 1.0
      num_threads: 4
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
    "execution_provider": {
      "default": "cpu",
      "description": "ONNX Runtime execution provider (requires libsherpa-onnx built with GPU support)",
      "enum": [
        "cpu",
        "cuda",
        "tensorrt"
      ],
      "type": "string"
    },
    "length_scale": {
      "default": 1.0,
      "description": "Duration control (alternative to speed)",
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
      "default": "./models/matcha-icefall-en_US-ljspeech",
      "description": "Path to Matcha model directory",
      "type": "string"
    },
    "noise_scale": {
      "default": 0.667,
      "description": "Voice variation control",
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
      "description": "Voice ID (0 for LJSpeech single-speaker model)",
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
