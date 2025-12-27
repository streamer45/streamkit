---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "Raw Audio"
description: "PacketType RawAudio structure"
---

`PacketType` id: `RawAudio`

Type system: `PacketType::RawAudio(AudioFormat)`

Runtime: `Packet::Audio(AudioFrame)`

## UI Metadata
- `label`: `Raw Audio`
- `color`: `#f39c12`
- `display_template`: `Raw Audio ({sample_rate|*}Hz, {channels|*}ch, {sample_format})`
- `compat: wildcard fields (sample_rate, channels, sample_format), color: `#f39c12``

## Structure
Raw audio is defined by an `AudioFormat` in the type system and carried as `Packet::Audio(AudioFrame)` at runtime.

### PacketType payload (`AudioFormat`)

| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `channels` | `integer (uint16)` | yes | — | min: `0`<br />max: `65535` |
| `sample_format` | `string enum[F32, S16Le]` | yes | — | Describes the specific format of raw audio data. |
| `sample_rate` | `integer (uint32)` | yes | — | min: `0` |

<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$defs": {
    "SampleFormat": {
      "description": "Describes the specific format of raw audio data.",
      "enum": [
        "F32",
        "S16Le"
      ],
      "type": "string"
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "description": "Contains the detailed metadata for a raw audio stream.",
  "properties": {
    "channels": {
      "format": "uint16",
      "maximum": 65535,
      "minimum": 0,
      "type": "integer"
    },
    "sample_format": {
      "$ref": "#/$defs/SampleFormat"
    },
    "sample_rate": {
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    }
  },
  "required": [
    "sample_rate",
    "channels",
    "sample_format"
  ],
  "title": "AudioFormat",
  "type": "object"
}
```

</details>

### Runtime payload (`AudioFrame`)

`AudioFrame` is optimized for zero-copy fan-out. It contains:

- `sample_rate` (u32)
- `channels` (u16)
- `samples` (interleaved f32 array)
- `metadata` (`PacketMetadata`, optional)
