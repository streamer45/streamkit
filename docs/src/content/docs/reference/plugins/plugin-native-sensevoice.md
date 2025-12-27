---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "plugin::native::sensevoice"
description: "Speech-to-text transcription using SenseVoice, a multilingual speech recognition model. Supports Chinese, English, Japanese, Korean, and Cantonese with automatic language detection. Requires 16kHz mono audio input."
---

`kind`: `plugin::native::sensevoice` (original kind: `sensevoice`)

Speech-to-text transcription using SenseVoice, a multilingual speech recognition model. Supports Chinese, English, Japanese, Korean, and Cantonese with automatic language detection. Requires 16kHz mono audio input.

Source: `plugins/native/sensevoice/target/release/libsensevoice.so`

## Categories
- `ml`
- `speech`
- `transcription`

## Pins
### Inputs
- `in` accepts `RawAudio(AudioFormat { sample_rate: 16000, channels: 1, sample_format: F32 })` (one)

### Outputs
- `out` produces `Transcription` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `execution_provider` | `string enum[cpu, cuda, tensorrt]` | no | `cpu` | Execution provider (cpu, cuda, tensorrt) |
| `language` | `string` | no | `auto` | Language code (auto, zh, en, ja, ko, yue) |
| `max_segment_duration_secs` | `number` | no | `30.0` | Maximum segment duration before forced transcription (seconds)<br />min: `5`<br />max: `120` |
| `min_silence_duration_ms` | `integer` | no | `700` | Minimum silence duration before transcription (milliseconds)<br />min: `100`<br />max: `5000` |
| `model_dir` | `string` | no | `models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2025-09-09` | Path to SenseVoice model directory. IMPORTANT: Input audio must be 16kHz mono f32. |
| `num_threads` | `integer` | no | `4` | Number of threads for inference<br />min: `1`<br />max: `16` |
| `use_itn` | `boolean` | no | `true` | Enable inverse text normalization (add punctuation) |
| `use_vad` | `boolean` | no | `true` | Enable VAD-based segmentation |
| `vad_model_path` | `string` | no | `models/silero_vad.onnx` | Path to Silero VAD ONNX model file |
| `vad_threshold` | `number` | no | `0.5` | VAD speech probability threshold (0.0-1.0)<br />min: `0`<br />max: `1` |

## Example Pipeline

```yaml
name: Speech-to-Text (SenseVoice)
description: Transcribes speech in multiple languages using SenseVoice
mode: oneshot
steps:
  - kind: streamkit::http_input

  - kind: containers::ogg::demuxer

  - kind: audio::opus::decoder

  - kind: audio::resampler
    params:
      chunk_frames: 960
      output_frame_size: 960
      target_sample_rate: 16000

  - kind: plugin::native::sensevoice
    params:
      model_dir: models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2025-09-09
      language: en
      use_itn: true
      num_threads: 4
      use_vad: true
      vad_model_path: models/silero_vad.onnx
      vad_threshold: 0.5
      min_silence_duration_ms: 700

  - kind: core::json_serialize
    params:
      pretty: false
      newline_delimited: true

  - kind: streamkit::http_output
    params:
      content_type: application/json
```


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "properties": {
    "execution_provider": {
      "default": "cpu",
      "description": "Execution provider (cpu, cuda, tensorrt)",
      "enum": [
        "cpu",
        "cuda",
        "tensorrt"
      ],
      "type": "string"
    },
    "language": {
      "default": "auto",
      "description": "Language code (auto, zh, en, ja, ko, yue)",
      "type": "string"
    },
    "max_segment_duration_secs": {
      "default": 30.0,
      "description": "Maximum segment duration before forced transcription (seconds)",
      "maximum": 120.0,
      "minimum": 5.0,
      "type": "number"
    },
    "min_silence_duration_ms": {
      "default": 700,
      "description": "Minimum silence duration before transcription (milliseconds)",
      "maximum": 5000,
      "minimum": 100,
      "type": "integer"
    },
    "model_dir": {
      "default": "models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2025-09-09",
      "description": "Path to SenseVoice model directory. IMPORTANT: Input audio must be 16kHz mono f32.",
      "type": "string"
    },
    "num_threads": {
      "default": 4,
      "description": "Number of threads for inference",
      "maximum": 16,
      "minimum": 1,
      "type": "integer"
    },
    "use_itn": {
      "default": true,
      "description": "Enable inverse text normalization (add punctuation)",
      "type": "boolean"
    },
    "use_vad": {
      "default": true,
      "description": "Enable VAD-based segmentation",
      "type": "boolean"
    },
    "vad_model_path": {
      "default": "models/silero_vad.onnx",
      "description": "Path to Silero VAD ONNX model file",
      "type": "string"
    },
    "vad_threshold": {
      "default": 0.5,
      "description": "VAD speech probability threshold (0.0-1.0)",
      "maximum": 1.0,
      "minimum": 0.0,
      "type": "number"
    }
  },
  "type": "object"
}
```

</details>
