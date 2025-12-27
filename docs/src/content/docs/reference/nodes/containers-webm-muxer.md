---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "containers::webm::muxer"
description: "Muxes Opus audio into a WebM container. Produces streamable WebM/Opus output compatible with web browsers."
---

`kind`: `containers::webm::muxer`

Muxes Opus audio into a WebM container. Produces streamable WebM/Opus output compatible with web browsers.

## Categories
- `containers`
- `webm`

## Pins
### Inputs
- `in` accepts `OpusAudio` (one)

### Outputs
- `out` produces `Binary` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `channels` | `integer (uint32)` | no | `2` | Number of audio channels (1 for mono, 2 for stereo)<br />min: `0` |
| `chunk_size` | `integer (uint)` | no | `65536` | The number of bytes to buffer before flushing to the output. Defaults to 65536.<br />min: `0` |
| `sample_rate` | `integer (uint32)` | no | `48000` | Audio sample rate in Hz<br />min: `0` |
| `streaming_mode` | `string` | no | — | — |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$defs": {
    "WebMStreamingMode": {
      "oneOf": [
        {
          "const": "live",
          "description": "Live streaming mode - optimized for real-time streaming, no duration/seeking info (default)",
          "type": "string"
        },
        {
          "const": "file",
          "description": "File mode - includes full duration and seeking information",
          "type": "string"
        }
      ]
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "channels": {
      "default": 2,
      "description": "Number of audio channels (1 for mono, 2 for stereo)",
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    },
    "chunk_size": {
      "default": 65536,
      "description": "The number of bytes to buffer before flushing to the output. Defaults to 65536.",
      "format": "uint",
      "minimum": 0,
      "type": "integer"
    },
    "sample_rate": {
      "default": 48000,
      "description": "Audio sample rate in Hz",
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    },
    "streaming_mode": {
      "$ref": "#/$defs/WebMStreamingMode",
      "description": "Streaming mode: \"live\" for real-time streaming (no duration), \"file\" for complete files with duration (default)"
    }
  },
  "title": "WebMMuxerConfig",
  "type": "object"
}
```

</details>
