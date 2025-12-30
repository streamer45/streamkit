---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "plugin::native::vad"
description: "Voice Activity Detection (VAD) using a high-performance ONNX model. Can output speech/silence events for downstream processing, or filter audio to pass only speech segments. Requires 16kHz mono audio input."
---

`kind`: `plugin::native::vad` (original kind: `vad`)

Voice Activity Detection (VAD) using a high-performance ONNX model. Can output speech/silence events for downstream processing, or filter audio to pass only speech segments. Requires 16kHz mono audio input.

Source: `plugins/native/vad/target/release/libvad.so`

## Categories
- `audio`
- `ml`

## Pins
### Inputs
- `in` accepts `RawAudio(AudioFormat { sample_rate: 16000, channels: 1, sample_format: F32 })` (one)

### Outputs
- `out` produces `Any` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `debug` | `boolean` | no | `false` | Enable debug logging from sherpa-onnx |
| `max_speech_duration_s` | `number` | no | `30.0` | Maximum speech duration in seconds |
| `min_silence_duration_s` | `number` | no | `0.5` | Minimum silence duration in seconds to trigger speech end |
| `min_speech_duration_s` | `number` | no | `0.25` | Minimum speech duration in seconds |
| `model_path` | `string` | no | `models/ten-vad.onnx` | Path to the ten-vad ONNX model |
| `num_threads` | `integer` | no | `1` | Number of threads for ONNX runtime |
| `output_mode` | `string enum[events, filtered_audio]` | no | `events` | Output mode: 'events' emits Custom packets on 'out' (type_id: plugin::native::vad/vad-event@1), 'filtered_audio' emits speech segments on 'out' |
| `provider` | `string` | no | `cpu` | ONNX execution provider (cpu, cuda, etc.) |
| `threshold` | `number` | no | `0.5` | VAD threshold (higher = more conservative)<br />min: `0`<br />max: `1` |
| `window_size` | `integer` | no | `512` | Window size for VAD processing (samples) |

## Example Pipeline

```yaml
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
#
# SPDX-License-Identifier: MPL-2.0

#
# skit:input_asset_tags=speech

name: Voice Activity Detection
description: Detects voice activity and outputs events as JSON
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

  - kind: plugin::native::vad
    params:
      model_path: models/ten-vad.onnx
      output_mode: events
      threshold: 0.5
      min_silence_duration_s: 0.5
      min_speech_duration_s: 0.25

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
    "debug": {
      "default": false,
      "description": "Enable debug logging from sherpa-onnx",
      "type": "boolean"
    },
    "max_speech_duration_s": {
      "default": 30.0,
      "description": "Maximum speech duration in seconds",
      "type": "number"
    },
    "min_silence_duration_s": {
      "default": 0.5,
      "description": "Minimum silence duration in seconds to trigger speech end",
      "type": "number"
    },
    "min_speech_duration_s": {
      "default": 0.25,
      "description": "Minimum speech duration in seconds",
      "type": "number"
    },
    "model_path": {
      "default": "models/ten-vad.onnx",
      "description": "Path to the ten-vad ONNX model",
      "type": "string"
    },
    "num_threads": {
      "default": 1,
      "description": "Number of threads for ONNX runtime",
      "type": "integer"
    },
    "output_mode": {
      "default": "events",
      "description": "Output mode: 'events' emits Custom packets on 'out' (type_id: plugin::native::vad/vad-event@1), 'filtered_audio' emits speech segments on 'out'",
      "enum": [
        "events",
        "filtered_audio"
      ],
      "type": "string"
    },
    "provider": {
      "default": "cpu",
      "description": "ONNX execution provider (cpu, cuda, etc.)",
      "type": "string"
    },
    "threshold": {
      "default": 0.5,
      "description": "VAD threshold (higher = more conservative)",
      "maximum": 1.0,
      "minimum": 0.0,
      "type": "number"
    },
    "window_size": {
      "default": 512,
      "description": "Window size for VAD processing (samples)",
      "type": "integer"
    }
  },
  "type": "object"
}
```

</details>
