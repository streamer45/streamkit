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
  "$defs": {
    "SplitMode": {
      "oneOf": [
        {
          "const": "sentences",
          "description": "Split on sentence boundaries (. ! ? etc.)",
          "type": "string"
        },
        {
          "const": "clauses",
          "description": "Split on sentences AND pauses (commas, dashes, semicolons) for natural streaming",
          "type": "string"
        },
        {
          "const": "words",
          "description": "Split after N words for lower latency (not recommended for TTS)",
          "type": "string"
        }
      ]
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "chunk_words": {
      "default": 5,
      "description": "Number of words per chunk (used in word mode)",
      "format": "uint",
      "minimum": 0,
      "type": "integer"
    },
    "min_length": {
      "default": 10,
      "description": "Minimum chunk length before emitting (used in sentence mode)",
      "format": "uint",
      "minimum": 0,
      "type": "integer"
    },
    "split_mode": {
      "$ref": "#/$defs/SplitMode",
      "description": "Splitting mode: \"sentences\" or \"words\""
    }
  },
  "title": "TextChunkerConfig",
  "type": "object"
}
```

</details>
