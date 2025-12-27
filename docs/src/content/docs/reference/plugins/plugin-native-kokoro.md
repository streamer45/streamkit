---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "plugin::native::kokoro"
description: "High-quality text-to-speech synthesis using the Kokoro TTS model. Supports 103 voices across Chinese and English with streaming output. Outputs 24kHz mono audio for real-time playback or further processing."
---

`kind`: `plugin::native::kokoro` (original kind: `kokoro`)

High-quality text-to-speech synthesis using the Kokoro TTS model. Supports 103 voices across Chinese and English with streaming output. Outputs 24kHz mono audio for real-time playback or further processing.

Source: `plugins/native/kokoro/target/release/libkokoro.so`

## Categories
- `audio`
- `tts`

## Pins
### Inputs
- `in` accepts `Text` (one)

### Outputs
- `out` produces `RawAudio(AudioFormat { sample_rate: 24000, channels: 1, sample_format: F32 })` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `emit_telemetry` | `boolean` | no | `false` | Emit out-of-band telemetry events (tts.start/tts.done) to the session telemetry bus |
| `execution_provider` | `string enum[cpu, cuda, tensorrt]` | no | `cpu` | ONNX Runtime execution provider (requires libsherpa-onnx built with GPU support) |
| `min_sentence_length` | `integer` | no | `10` | Minimum chars before TTS generation<br />min: `1` |
| `model_dir` | `string` | yes | `./models/kokoro-multi-lang-v1_1` | Path to Kokoro model directory |
| `num_threads` | `integer` | no | `4` | CPU threads for inference<br />min: `1`<br />max: `16` |
| `speaker_id` | `integer` | no | `50` | Voice ID (0-102 for v1.1)<br />min: `0`<br />max: `102` |
| `speed` | `number` | no | `1.0` | Speech speed multiplier<br />min: `0.5`<br />max: `2` |
| `telemetry_preview_chars` | `integer` | no | `80` | Maximum characters of text preview to include in telemetry events (0 = omit preview)<br />min: `0`<br />max: `1000` |

## Example Pipeline

```yaml
name: Text-to-Speech (Kokoro)
description: Synthesizes speech from text using Kokoro
mode: oneshot
steps:
  - kind: streamkit::http_input
  - kind: core::text_chunker
    params:
      min_length: 10
  - kind: plugin::native::kokoro
    params:
      model_dir: "models/kokoro-multi-lang-v1_1"
      speaker_id: 0
      speed: 1.0
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
    "emit_telemetry": {
      "default": false,
      "description": "Emit out-of-band telemetry events (tts.start/tts.done) to the session telemetry bus",
      "type": "boolean"
    },
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
    "min_sentence_length": {
      "default": 10,
      "description": "Minimum chars before TTS generation",
      "minimum": 1,
      "type": "integer"
    },
    "model_dir": {
      "default": "./models/kokoro-multi-lang-v1_1",
      "description": "Path to Kokoro model directory",
      "type": "string"
    },
    "num_threads": {
      "default": 4,
      "description": "CPU threads for inference",
      "maximum": 16,
      "minimum": 1,
      "type": "integer"
    },
    "speaker_id": {
      "default": 50,
      "description": "Voice ID (0-102 for v1.1)",
      "maximum": 102,
      "minimum": 0,
      "type": "integer"
    },
    "speed": {
      "default": 1.0,
      "description": "Speech speed multiplier",
      "maximum": 2.0,
      "minimum": 0.5,
      "type": "number"
    },
    "telemetry_preview_chars": {
      "default": 80,
      "description": "Maximum characters of text preview to include in telemetry events (0 = omit preview)",
      "maximum": 1000,
      "minimum": 0,
      "type": "integer"
    }
  },
  "required": [
    "model_dir"
  ],
  "type": "object"
}
```

</details>
