---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "plugin::native::helsinki"
description: "Neural machine translation using Helsinki-NLP OPUS-MT models. Supports bidirectional EN<->ES translation with Apache 2.0 licensed models. Powered by Candle (pure Rust ML framework)."
---

`kind`: `plugin::native::helsinki` (original kind: `helsinki`)

Neural machine translation using Helsinki-NLP OPUS-MT models. Supports bidirectional EN<->ES translation with Apache 2.0 licensed models. Powered by Candle (pure Rust ML framework).

Source: `plugins/native/helsinki/target/release/libhelsinki.so`

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
| `device` | `string enum[cpu, cuda, auto]` | no | `cpu` | Device to use: 'cpu', 'cuda', or 'auto' |
| `device_index` | `integer` | no | `0` | GPU device index (only used when device is 'cuda')<br />min: `0`<br />max: `7` |
| `max_length` | `integer` | no | `512` | Maximum output sequence length<br />min: `32`<br />max: `2048` |
| `model_dir` | `string` | no | `models/opus-mt-en-es` | Path to model directory containing safetensors and tokenizer files |
| `source_language` | `string enum[en, es]` | no | `en` | Source language code: 'en' (English) or 'es' (Spanish) |
| `target_language` | `string enum[en, es]` | no | `es` | Target language code: 'en' (English) or 'es' (Spanish) |
| `warmup` | `boolean` | no | `false` | If true, run a small warmup translation during initialization to reduce first-request latency |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "properties": {
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
    "max_length": {
      "default": 512,
      "description": "Maximum output sequence length",
      "maximum": 2048,
      "minimum": 32,
      "type": "integer"
    },
    "model_dir": {
      "default": "models/opus-mt-en-es",
      "description": "Path to model directory containing safetensors and tokenizer files",
      "type": "string"
    },
    "source_language": {
      "default": "en",
      "description": "Source language code: 'en' (English) or 'es' (Spanish)",
      "enum": [
        "en",
        "es"
      ],
      "type": "string"
    },
    "target_language": {
      "default": "es",
      "description": "Target language code: 'en' (English) or 'es' (Spanish)",
      "enum": [
        "en",
        "es"
      ],
      "type": "string"
    },
    "warmup": {
      "default": false,
      "description": "If true, run a small warmup translation during initialization to reduce first-request latency",
      "type": "boolean"
    }
  },
  "type": "object"
}
```

</details>
