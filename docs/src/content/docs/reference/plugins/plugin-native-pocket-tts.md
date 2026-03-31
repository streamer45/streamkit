---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "plugin::native::pocket-tts"
description: "Lightweight CPU TTS using Kyutai Pocket TTS (Candle). English-only voices with streaming output. Outputs 24kHz mono audio."
---

`kind`: `plugin::native::pocket-tts` (original kind: `pocket-tts`)

Lightweight CPU TTS using Kyutai Pocket TTS (Candle). English-only voices with streaming output. Outputs 24kHz mono audio.

Source: `target/plugins/release/libpocket_tts.so`

## Categories
- `audio`
- `tts`
- `ml`

## Pins
### Inputs
- `in` accepts `Text, Binary` (one)
- `in_0` accepts `Text, Binary` (one)
- `in_1` accepts `Binary, Text` (one)

### Outputs
- `out` produces `RawAudio(AudioFormat { sample_rate: 24000, channels: 1, sample_format: F32 })` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `config_path` | `null | string` | no | `null` | Optional config YAML path for custom variants/offline use |
| `eos_threshold` | `number` | no | `-4.0` | End-of-sequence threshold (more negative = longer output)<br />min: `-10`<br />max: `0` |
| `lsd_decode_steps` | `integer` | no | `1` | LSD decode steps (higher = better quality, slower)<br />min: `1`<br />max: `8` |
| `min_sentence_length` | `integer` | no | `10` | Minimum chars before triggering TTS<br />min: `1` |
| `noise_clamp` | `null | number` | no | `null` | Optional noise clamp (null disables) |
| `quantized` | `boolean` | no | `false` | Enable int8 quantized weights (requires plugin built with feature 'quantized') |
| `temperature` | `number` | no | `0.7` | Sampling temperature (higher = more variation)<br />min: `0.1`<br />max: `2` |
| `tokenizer_path` | `null | string` | no | `null` | Local tokenizer path for offline loading |
| `variant` | `string` | no | `b6369a24` | Model variant (config in pocket-tts crate) |
| `voice` | `string` | no | `alba` | Voice name, local .wav/.safetensors, hf:// URL, or base64 audio |
| `voice_embeddings_dir` | `null | string` | no | `null` | Directory with predefined voice embeddings (alba, marius, ...) |
| `weights_path` | `null | string` | no | `null` | Local weights path for offline loading |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "properties": {
    "config_path": {
      "default": null,
      "description": "Optional config YAML path for custom variants/offline use",
      "type": [
        "string",
        "null"
      ]
    },
    "eos_threshold": {
      "default": -4.0,
      "description": "End-of-sequence threshold (more negative = longer output)",
      "maximum": 0.0,
      "minimum": -10.0,
      "type": "number"
    },
    "lsd_decode_steps": {
      "default": 1,
      "description": "LSD decode steps (higher = better quality, slower)",
      "maximum": 8,
      "minimum": 1,
      "type": "integer"
    },
    "min_sentence_length": {
      "default": 10,
      "description": "Minimum chars before triggering TTS",
      "minimum": 1,
      "type": "integer"
    },
    "noise_clamp": {
      "default": null,
      "description": "Optional noise clamp (null disables)",
      "type": [
        "number",
        "null"
      ]
    },
    "quantized": {
      "default": false,
      "description": "Enable int8 quantized weights (requires plugin built with feature 'quantized')",
      "type": "boolean"
    },
    "temperature": {
      "default": 0.7,
      "description": "Sampling temperature (higher = more variation)",
      "maximum": 2.0,
      "minimum": 0.1,
      "type": "number"
    },
    "tokenizer_path": {
      "default": null,
      "description": "Local tokenizer path for offline loading",
      "type": [
        "string",
        "null"
      ]
    },
    "variant": {
      "default": "b6369a24",
      "description": "Model variant (config in pocket-tts crate)",
      "type": "string"
    },
    "voice": {
      "default": "alba",
      "description": "Voice name, local .wav/.safetensors, hf:// URL, or base64 audio",
      "type": "string"
    },
    "voice_embeddings_dir": {
      "default": null,
      "description": "Directory with predefined voice embeddings (alba, marius, ...)",
      "type": [
        "string",
        "null"
      ]
    },
    "weights_path": {
      "default": null,
      "description": "Local weights path for offline loading",
      "type": [
        "string",
        "null"
      ]
    }
  },
  "type": "object"
}
```

</details>
