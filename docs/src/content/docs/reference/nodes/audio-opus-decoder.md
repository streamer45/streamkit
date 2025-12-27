---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "audio::opus::decoder"
description: "Decodes Opus-compressed audio packets into raw PCM samples. Opus is the preferred codec for real-time audio due to its low latency and excellent quality across all bitrates."
---

`kind`: `audio::opus::decoder`

Decodes Opus-compressed audio packets into raw PCM samples. Opus is the preferred codec for real-time audio due to its low latency and excellent quality across all bitrates.

## Categories
- `audio`
- `codecs`
- `opus`

## Pins
### Inputs
- `in` accepts `OpusAudio` (one)

### Outputs
- `out` produces `RawAudio(AudioFormat { sample_rate: 48000, channels: 1, sample_format: F32 })` (broadcast)

## Parameters
No parameters.


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "OpusDecoderConfig",
  "type": "object"
}
```

</details>
