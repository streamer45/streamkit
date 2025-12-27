---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "audio::opus::encoder"
description: "Encodes raw PCM audio into Opus-compressed packets. Configurable bitrate, application mode (VoIP/audio), and complexity settings. Ideal for streaming and real-time communication."
---

`kind`: `audio::opus::encoder`

Encodes raw PCM audio into Opus-compressed packets. Configurable bitrate, application mode (VoIP/audio), and complexity settings. Ideal for streaming and real-time communication.

## Categories
- `audio`
- `codecs`
- `opus`

## Pins
### Inputs
- `in` accepts `RawAudio(AudioFormat { sample_rate: 48000, channels: 1, sample_format: F32 })` (one)

### Outputs
- `out` produces `OpusAudio` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `bitrate` | `integer` | no | `64000` | min: `6000`<br />max: `510000` |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "bitrate": {
      "default": 64000,
      "maximum": 510000,
      "minimum": 6000,
      "multipleOf": 1000,
      "tunable": false,
      "type": "integer"
    }
  },
  "title": "OpusEncoderConfig",
  "type": "object"
}
```

</details>
