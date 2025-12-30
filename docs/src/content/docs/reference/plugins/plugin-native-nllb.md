---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "plugin::native::nllb"
description: "Neural machine translation using Meta's NLLB (No Language Left Behind) model. Supports translation between 200+ languages. Accepts both text and transcription packets."
---

`kind`: `plugin::native::nllb` (original kind: `nllb`)

Neural machine translation using Meta's NLLB (No Language Left Behind) model. Supports translation between 200+ languages. Accepts both text and transcription packets.

Source: `plugins/native/nllb/target/release/libnllb.so`

## Categories
- `ml`
- `translation`
- `text`

## Pins
### Inputs
- `in` accepts `Text, Transcription` (one)

### Outputs
- `out` produces `Text` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `beam_size` | `integer` | no | `1` | Beam search size (1 = greedy/fast, 4 = better quality/slower)<br />min: `1`<br />max: `10` |
| `device` | `string enum[cpu, cuda, auto]` | no | `cpu` | Device to use: 'cpu', 'cuda', or 'auto' |
| `device_index` | `integer` | no | `0` | GPU device index (only used when device is 'cuda')<br />min: `0`<br />max: `7` |
| `model_path` | `string` | no | `models/nllb-200-distilled-600M-ct2-int8` | Path to CTranslate2 model directory (see README for conversion instructions) |
| `num_threads` | `integer` | no | `0` | Number of threads (0 = auto, recommended 4-8 for real-time)<br />min: `0`<br />max: `32` |
| `source_language` | `string` | no | `eng_Latn` | Source language code in NLLB format (e.g., 'eng_Latn', 'spa_Latn', 'zho_Hans') |
| `target_language` | `string` | no | `spa_Latn` | Target language code in NLLB format (e.g., 'eng_Latn', 'spa_Latn', 'zho_Hans') |

## Example Pipeline

```yaml
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
#
# SPDX-License-Identifier: MPL-2.0

#
# skit:input_asset_tags=speech

name: Speech Translation (English → Spanish)
description: Translates English speech into Spanish speech
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
      model_path: models/ggml-tiny.en-q5_1.bin
      language: en
      vad_model_path: models/silero_vad.onnx
      vad_threshold: 0.5
      min_silence_duration_ms: 700
      max_segment_duration_secs: 30.0

  - kind: plugin::native::nllb
    params:
      model_path: models/nllb-200-distilled-600M-ct2-int8
      source_language: eng_Latn
      target_language: spa_Latn
      compute_type: int8
      beam_size: 1
      num_threads: 4

  - kind: plugin::native::piper
    params:
      model_dir: models/vits-piper-es_MX-claude-high
      speed: 1.0
      num_threads: 4

  - kind: audio::resampler
    params:
      chunk_frames: 960
      output_frame_size: 960
      target_sample_rate: 48000

  - kind: audio::opus::encoder
    params:
      bitrate: 64000
      frame_size: 960

  - kind: containers::webm::muxer
    params:
      channels: 1
      chunk_size: 65536
      sample_rate: 48000

  - kind: streamkit::http_output
    params:
      content_type: audio/webm; codecs="opus"
```


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "properties": {
    "beam_size": {
      "default": 1,
      "description": "Beam search size (1 = greedy/fast, 4 = better quality/slower)",
      "maximum": 10,
      "minimum": 1,
      "type": "integer"
    },
    "device": {
      "default": "cpu",
      "description": "Device to use: 'cpu', 'cuda', or 'auto'",
      "enum": [
        "cpu",
        "cuda",
        "auto"
      ],
      "type": "string"
    },
    "device_index": {
      "default": 0,
      "description": "GPU device index (only used when device is 'cuda')",
      "maximum": 7,
      "minimum": 0,
      "type": "integer"
    },
    "model_path": {
      "default": "models/nllb-200-distilled-600M-ct2-int8",
      "description": "Path to CTranslate2 model directory (see README for conversion instructions)",
      "type": "string"
    },
    "num_threads": {
      "default": 0,
      "description": "Number of threads (0 = auto, recommended 4-8 for real-time)",
      "maximum": 32,
      "minimum": 0,
      "type": "integer"
    },
    "source_language": {
      "default": "eng_Latn",
      "description": "Source language code in NLLB format (e.g., 'eng_Latn', 'spa_Latn', 'zho_Hans')",
      "type": "string"
    },
    "target_language": {
      "default": "spa_Latn",
      "description": "Target language code in NLLB format (e.g., 'eng_Latn', 'spa_Latn', 'zho_Hans')",
      "type": "string"
    }
  },
  "type": "object"
}
```

</details>
