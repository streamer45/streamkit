---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "plugin::native::supertonic"
description: "Multilingual text-to-speech using the Supertonic TTS engine. Supports English, Korean, Spanish, Portuguese, and French with 10 voice styles."
---

`kind`: `plugin::native::supertonic` (original kind: `supertonic`)

Multilingual text-to-speech using the Supertonic TTS engine. Supports English, Korean, Spanish, Portuguese, and French with 10 voice styles.

Source: `target/plugins/release/libsupertonic.so`

## Categories
- `audio`
- `tts`
- `ml`

## Pins
### Inputs
- `in` accepts `Text` (one)

### Outputs

- `out` produces `RawAudio(AudioFormat { sample_rate: 22050, channels: 1, sample_format: F32 })` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `model_dir` | `string` | yes | `./models/supertonic-v2-onnx` | Path to Supertonic ONNX model directory |
| `lang` | `string` | no | `en` | Language code: `en`, `ko`, `es`, `pt`, or `fr` |
| `voice_style` | `string` | no | `M1` | Voice style name (`M1`-`M5`, `F1`-`F5`) or path to a `.json` file |
| `voice_styles_dir` | `string` | no | — | Directory containing named voice style `.json` files |
| `total_step` | `integer` | no | `5` | Denoising steps; higher is better quality but slower |
| `speed` | `number` | no | `1.05` | Speech speed multiplier |
| `silence_duration` | `number` | no | `0.3` | Silence between chunks in seconds |
| `min_sentence_length` | `integer` | no | `10` | Minimum chars before TTS generation |
| `emit_telemetry` | `boolean` | no | `false` | Emit out-of-band TTS telemetry events |
| `telemetry_preview_chars` | `integer` | no | `80` | Maximum characters of text preview in telemetry (`0` = omit) |

<details>
<summary>Raw JSON Schema</summary>

```json
{
  "type": "object",
  "required": ["model_dir"],
  "properties": {
    "model_dir": { "type": "string", "default": "./models/supertonic-v2-onnx" },
    "lang": { "type": "string", "default": "en", "enum": ["en", "ko", "es", "pt", "fr"] },
    "voice_style": { "type": "string", "default": "M1" },
    "voice_styles_dir": { "type": "string" },
    "total_step": { "type": "integer", "default": 5, "minimum": 1, "maximum": 20 },
    "speed": { "type": "number", "default": 1.05, "minimum": 0.5, "maximum": 2.0 },
    "silence_duration": { "type": "number", "default": 0.3, "minimum": 0.0, "maximum": 2.0 },
    "min_sentence_length": { "type": "integer", "default": 10, "minimum": 1 },
    "emit_telemetry": { "type": "boolean", "default": false },
    "telemetry_preview_chars": { "type": "integer", "default": 80, "minimum": 0, "maximum": 1000 }
  }
}
```
</details>
