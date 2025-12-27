---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "core::passthrough"
description: "Forwards packets unchanged. Useful for pipeline debugging, branching, or as a placeholder during development."
---

`kind`: `core::passthrough`

Forwards packets unchanged. Useful for pipeline debugging, branching, or as a placeholder during development.

## Categories
- `core`

## Pins
### Inputs
- `in` accepts `Any` (one)

### Outputs
- `out` produces `Passthrough` (broadcast)

## Parameters
No parameters.


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "PassthroughConfig",
  "type": "object"
}
```

</details>
