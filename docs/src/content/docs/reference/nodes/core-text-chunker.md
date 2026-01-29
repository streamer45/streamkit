---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "core::text_chunker"
description: "Splits text into smaller chunks at sentence or clause boundaries. Essential for streaming TTS where text should be spoken as it arrives rather than waiting for complete paragraphs."
---

`kind`: `core::text_chunker`

Splits text into smaller chunks at sentence or clause boundaries. Essential for streaming TTS where text should be spoken as it arrives rather than waiting for complete paragraphs.

## Categories
- `core`
- `text`

## Pins
### Inputs
- `in` accepts `Text, Binary` (one)

### Outputs
- `out` produces `Text` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `chunk_words` | `integer (uint)` | no | `5` | Number of words per chunk (used in word mode)<br />min: `0` |
| `min_length` | `integer (uint)` | no | `10` | Minimum chunk length before emitting (used in sentence mode)<br />min: `0` |
| `split_mode` | `string` | no | — | — |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "TextChunkerConfig",
  "type": "object",
  "properties": {
    "split_mode": {
      "description": "Splitting mode: \"sentences\" or \"words\"",
      "$ref": "#/$defs/SplitMode"
    },
    "min_length": {
      "description": "Minimum chunk length before emitting (used in sentence mode)",
      "type": "integer",
      "format": "uint",
      "minimum": 0,
      "default": 10
    },
    "chunk_words": {
      "description": "Number of words per chunk (used in word mode)",
      "type": "integer",
      "format": "uint",
      "minimum": 0,
      "default": 5
    }
  },
  "$defs": {
    "SplitMode": {
      "oneOf": [
        {
          "description": "Split on sentence boundaries (. ! ? etc.)",
          "type": "string",
          "const": "sentences"
        },
        {
          "description": "Split on sentences AND pauses (commas, dashes, semicolons) for natural streaming",
          "type": "string",
          "const": "clauses"
        },
        {
          "description": "Split after N words for lower latency (not recommended for TTS)",
          "type": "string",
          "const": "words"
        }
      ]
    }
  }
}
```

</details>
