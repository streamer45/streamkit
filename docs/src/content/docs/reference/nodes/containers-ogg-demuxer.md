---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "containers::ogg::demuxer"
description: "Demuxes Ogg containers to extract Opus audio packets. Accepts binary Ogg data and outputs Opus-encoded audio frames."
---

`kind`: `containers::ogg::demuxer`

Demuxes Ogg containers to extract Opus audio packets. Accepts binary Ogg data and outputs Opus-encoded audio frames.

## Categories
- `containers`
- `ogg`

## Pins
### Inputs
- `in` accepts `Binary` (one)

### Outputs
- `out` produces `OpusAudio` (broadcast)

## Parameters
No parameters.


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "SymphoniaOggDemuxerConfig",
  "type": "object"
}
```

</details>
