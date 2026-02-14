---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "containers::ogg::muxer"
description: "Muxes Opus audio packets into an Ogg container. Produces streamable Ogg/Opus output for playback or storage."
---

`kind`: `containers::ogg::muxer`

Muxes Opus audio packets into an Ogg container. Produces streamable Ogg/Opus output for playback or storage.

## Categories
- `containers`
- `ogg`

## Pins
### Inputs
- `in` accepts `EncodedAudio(EncodedAudioFormat { codec: Opus })` (one)

### Outputs
- `out` produces `Binary` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `channels` | `integer (uint8)` | no | `1` | Number of audio channels (1 for mono, 2 for stereo). Defaults to 1.<br />min: `0`<br />max: `255` |
| `chunk_size` | `integer (uint)` | no | `65536` | The number of bytes to buffer before flushing to the output. Defaults to 65536.<br />min: `0` |
| `codec` | `string enum[opus]` | no | — | — |
| `stream_serial` | `integer (uint32)` | no | `0` | min: `0` |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "OggMuxerConfig",
  "type": "object",
  "properties": {
    "stream_serial": {
      "type": "integer",
      "format": "uint32",
      "minimum": 0,
      "default": 0
    },
    "codec": {
      "$ref": "#/$defs/OggMuxerCodec"
    },
    "channels": {
      "description": "Number of audio channels (1 for mono, 2 for stereo). Defaults to 1.",
      "type": "integer",
      "format": "uint8",
      "minimum": 0,
      "maximum": 255,
      "default": 1
    },
    "chunk_size": {
      "description": "The number of bytes to buffer before flushing to the output. Defaults to 65536.",
      "type": "integer",
      "format": "uint",
      "minimum": 0,
      "default": 65536
    }
  },
  "$defs": {
    "OggMuxerCodec": {
      "type": "string",
      "enum": [
        "opus"
      ]
    }
  }
}
```

</details>
