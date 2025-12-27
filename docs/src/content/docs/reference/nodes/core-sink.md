---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "core::sink"
description: "Accepts packets and discards them. Useful for terminating side-branches (e.g., telemetry taps) without affecting the main pipeline."
---

`kind`: `core::sink`

Accepts packets and discards them. Useful for terminating side-branches (e.g., telemetry taps) without affecting the main pipeline.

## Categories
- `core`
- `observability`

## Pins
### Inputs
- `in` accepts `Any` (one)

### Outputs
No outputs.

## Parameters
No parameters.


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "SinkConfig",
  "type": "object"
}
```

</details>
