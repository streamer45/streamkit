---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "Transcription"
description: "PacketType Transcription structure"
---

`PacketType` id: `Transcription`

Type system: `PacketType::Transcription`

Runtime: `Packet::Transcription(Arc<TranscriptionData>)`

## UI Metadata
- `label`: `Transcription`
- `color`: `#9b59b6`
- `compat: exact, color: `#9b59b6``

## Structure
Transcriptions are carried as `Packet::Transcription(Arc<TranscriptionData>)`.

| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `language` | `null | string` | no | — | Detected or specified language code (e.g., "en", "es", "fr") |
| `metadata` | `null | object` | no | — | Optional timing metadata for the entire transcription |
| `segments` | `array<object>` | yes | — | Individual segments with timing information |
| `text` | `string` | yes | — | The full transcribed text (concatenation of all segments) |

#### `segments` fields

| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `confidence` | `null | number (float)` | no | — | Confidence score (0.0 - 1.0), if available |
| `end_time_ms` | `integer (uint64)` | yes | — | End time in milliseconds<br />min: `0` |
| `start_time_ms` | `integer (uint64)` | yes | — | Start time in milliseconds<br />min: `0` |
| `text` | `string` | yes | — | The transcribed text for this segment |

<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "TranscriptionData",
  "description": "Structured transcription data with timing and metadata.",
  "type": "object",
  "properties": {
    "text": {
      "description": "The full transcribed text (concatenation of all segments)",
      "type": "string"
    },
    "segments": {
      "description": "Individual segments with timing information",
      "type": "array",
      "items": {
        "$ref": "#/$defs/TranscriptionSegment"
      }
    },
    "language": {
      "description": "Detected or specified language code (e.g., \"en\", \"es\", \"fr\")",
      "type": [
        "string",
        "null"
      ]
    },
    "metadata": {
      "description": "Optional timing metadata for the entire transcription",
      "anyOf": [
        {
          "$ref": "#/$defs/PacketMetadata"
        },
        {
          "type": "null"
        }
      ]
    }
  },
  "required": [
    "text",
    "segments"
  ],
  "$defs": {
    "TranscriptionSegment": {
      "description": "A segment of transcribed text with timing information.",
      "type": "object",
      "properties": {
        "text": {
          "description": "The transcribed text for this segment",
          "type": "string"
        },
        "start_time_ms": {
          "description": "Start time in milliseconds",
          "type": "integer",
          "format": "uint64",
          "minimum": 0
        },
        "end_time_ms": {
          "description": "End time in milliseconds",
          "type": "integer",
          "format": "uint64",
          "minimum": 0
        },
        "confidence": {
          "description": "Confidence score (0.0 - 1.0), if available",
          "type": [
            "number",
            "null"
          ],
          "format": "float"
        }
      },
      "required": [
        "text",
        "start_time_ms",
        "end_time_ms"
      ]
    },
    "PacketMetadata": {
      "description": "Optional timing and sequencing metadata that can be attached to packets.\nUsed for pacing, synchronization, and A/V alignment. See `timing` module for\ncanonical semantics (media-time epoch, monotonicity, and preservation rules).",
      "type": "object",
      "properties": {
        "timestamp_us": {
          "description": "Absolute timestamp in microseconds (presentation time)",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint64",
          "minimum": 0
        },
        "duration_us": {
          "description": "Duration of this packet/frame in microseconds",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint64",
          "minimum": 0
        },
        "sequence": {
          "description": "Sequence number for ordering and detecting loss",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint64",
          "minimum": 0
        },
        "keyframe": {
          "description": "Keyframe flag for encoded video packets (and raw frames if applicable)",
          "type": [
            "boolean",
            "null"
          ]
        }
      }
    }
  }
}
```

</details>
