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
- `in` accepts `EncodedAudio(EncodedAudioFormat { codec: Opus })` (one)

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
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "WebMMuxerConfig",
  "type": "object",
  "properties": {
    "sample_rate": {
      "description": "Audio sample rate in Hz",
      "type": "integer",
      "format": "uint32",
      "minimum": 0,
      "default": 48000
    },
    "channels": {
      "description": "Number of audio channels (1 for mono, 2 for stereo)",
      "type": "integer",
      "format": "uint32",
      "minimum": 0,
      "default": 2
    },
    "chunk_size": {
      "description": "The number of bytes to buffer before flushing to the output. Defaults to 65536.",
      "type": "integer",
      "format": "uint",
      "minimum": 0,
      "default": 65536
    },
    "streaming_mode": {
      "description": "Streaming mode: \"live\" for real-time streaming (no duration), \"file\" for complete files with duration (default)",
      "$ref": "#/$defs/WebMStreamingMode"
    }
  },
  "$defs": {
    "WebMStreamingMode": {
      "oneOf": [
        {
          "description": "Live streaming mode - optimized for real-time streaming, no duration/seeking info (default)",
          "type": "string",
          "const": "live"
        },
        {
          "description": "File mode - includes full duration and seeking information",
          "type": "string",
          "const": "file"
        }
      ]
    }
  }
}
```

</details>
