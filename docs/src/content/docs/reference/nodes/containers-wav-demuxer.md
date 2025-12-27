---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "containers::wav::demuxer"
description: "Demuxes WAV audio files to raw PCM samples. Accepts binary WAV data and outputs 48kHz stereo f32 audio."
---

`kind`: `containers::wav::demuxer`

Demuxes WAV audio files to raw PCM samples. Accepts binary WAV data and outputs 48kHz stereo f32 audio.

## Categories
- `containers`
- `wav`

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
  "title": "WavDemuxerConfig",
  "type": "object"
}
```

</details>
