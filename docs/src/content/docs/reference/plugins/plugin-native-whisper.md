---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "plugin::native::whisper"
description: "Real-time speech-to-text transcription using OpenAI's Whisper model. Features VAD-based segmentation for natural speech boundaries, GPU acceleration support, and streaming output. Requires 16kHz mono audio input."
---

`kind`: `plugin::native::whisper` (original kind: `whisper`)

Real-time speech-to-text transcription using OpenAI's Whisper model. Features VAD-based segmentation for natural speech boundaries, GPU acceleration support, and streaming output. Requires 16kHz mono audio input.

Source: `plugins/native/whisper/target/release/libwhisper.so`

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
| `emit_vad_events` | `boolean` | no | `false` | Emit VAD speech start/end out-of-band to the telemetry bus (does not flow through graph pins). |
| `gpu_device` | `integer` | no | `0` | GPU device ID to use (0 = first GPU, 1 = second GPU, etc.)<br />min: `0`<br />max: `7` |
| `language` | `string` | no | `en` | Language code (e.g., 'en', 'es', 'fr') |
| `max_segment_duration_secs` | `number` | no | `30.0` | Maximum segment duration before forced transcription (seconds)<br />min: `5`<br />max: `120` |
| `min_silence_duration_ms` | `integer` | no | `700` | Minimum silence duration before transcription (milliseconds)<br />min: `100`<br />max: `5000` |
| `model_path` | `string` | no | `models/ggml-base.en-q5_1.bin` | Path to Whisper GGML model file (relative to repo root). IMPORTANT: Input audio must be 16kHz mono f32. |
| `n_threads` | `integer` | no | `0` | Number of threads for decoding (0 = auto: min(4, num_cores), 8-12 recommended for modern CPUs)<br />min: `0`<br />max: `32` |
| `suppress_blank` | `boolean` | no | `true` | Suppress blank/silent audio segments |
| `suppress_non_speech_tokens` | `boolean` | no | `true` | Suppress non-speech tokens like [BLANK_AUDIO], [MUSIC], [APPLAUSE], etc. |
| `use_gpu` | `boolean` | no | `false` | Enable GPU acceleration (requires whisper.cpp built with CUDA support) |
| `vad_model_path` | `string` | no | `models/silero_vad.onnx` | Path to Silero VAD ONNX model file |
| `vad_threshold` | `number` | no | `0.5` | VAD speech probability threshold (0.0-1.0)<br />min: `0`<br />max: `1` |

## Example Pipeline

```yaml
#
# skit:input_asset_tags=speech

name: Speech-to-Text (Whisper)
description: Transcribes speech to text using Whisper
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

  - kind: plugin::native::whisper
    params:
      model_path: models/ggml-base.en-q5_1.bin
      language: en
      vad_model_path: models/silero_vad.onnx
      vad_threshold: 0.5
      min_silence_duration_ms: 700
      max_segment_duration_secs: 30.0

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
    "emit_vad_events": {
      "default": false,
      "description": "Emit VAD speech start/end out-of-band to the telemetry bus (does not flow through graph pins).",
      "type": "boolean"
    },
    "gpu_device": {
      "default": 0,
      "description": "GPU device ID to use (0 = first GPU, 1 = second GPU, etc.)",
      "maximum": 7,
      "minimum": 0,
      "type": "integer"
    },
    "language": {
      "default": "en",
      "description": "Language code (e.g., 'en', 'es', 'fr')",
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
    "model_path": {
      "default": "models/ggml-base.en-q5_1.bin",
      "description": "Path to Whisper GGML model file (relative to repo root). IMPORTANT: Input audio must be 16kHz mono f32.",
      "type": "string"
    },
    "n_threads": {
      "default": 0,
      "description": "Number of threads for decoding (0 = auto: min(4, num_cores), 8-12 recommended for modern CPUs)",
      "maximum": 32,
      "minimum": 0,
      "type": "integer"
    },
    "suppress_blank": {
      "default": true,
      "description": "Suppress blank/silent audio segments",
      "type": "boolean"
    },
    "suppress_non_speech_tokens": {
      "default": true,
      "description": "Suppress non-speech tokens like [BLANK_AUDIO], [MUSIC], [APPLAUSE], etc.",
      "type": "boolean"
    },
    "use_gpu": {
      "default": false,
      "description": "Enable GPU acceleration (requires whisper.cpp built with CUDA support)",
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
