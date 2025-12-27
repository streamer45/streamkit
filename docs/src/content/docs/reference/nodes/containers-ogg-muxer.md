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
- `in` accepts `OpusAudio` (one)

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
  "$defs": {
    "OggMuxerCodec": {
      "enum": [
        "opus"
      ],
      "type": "string"
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "channels": {
      "default": 1,
      "description": "Number of audio channels (1 for mono, 2 for stereo). Defaults to 1.",
      "format": "uint8",
      "maximum": 255,
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
    "codec": {
      "$ref": "#/$defs/OggMuxerCodec"
    },
    "stream_serial": {
      "default": 0,
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    }
  },
  "title": "OggMuxerConfig",
  "type": "object"
}
```

</details>
