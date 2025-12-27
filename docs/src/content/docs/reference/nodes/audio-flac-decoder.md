---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "audio::flac::decoder"
description: "Decodes FLAC audio data to raw PCM samples. Accepts binary FLAC data and outputs 48kHz stereo f32 audio."
---

`kind`: `audio::flac::decoder`

Decodes FLAC audio data to raw PCM samples. Accepts binary FLAC data and outputs 48kHz stereo f32 audio.

## Categories
- `audio`
- `codecs`
- `flac`

## Pins
### Inputs
- `in` accepts `Binary` (one)

### Outputs
- `out` produces `RawAudio(AudioFormat { sample_rate: 48000, channels: 2, sample_format: F32 })` (broadcast)

## Parameters
No parameters.


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "FlacDecoderConfig",
  "type": "object"
}
```

</details>
