---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "core::file_reader"
description: "Reads binary data from a file and emits it as packets. Supports configurable chunk sizes for streaming large files."
---

`kind`: `core::file_reader`

Reads binary data from a file and emits it as packets. Supports configurable chunk sizes for streaming large files.

## Categories
- `core`
- `io`

## Pins
### Inputs
No inputs.

### Outputs
- `out` produces `Binary` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `chunk_size` | `integer (uint)` | no | `8192` | Size of chunks to read (default: 8192 bytes)<br />min: `0` |
| `path` | `string` | yes | — | Path to the file to read |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "description": "Configuration for the FileReadNode",
  "properties": {
    "chunk_size": {
      "default": 8192,
      "description": "Size of chunks to read (default: 8192 bytes)",
      "format": "uint",
      "minimum": 0,
      "type": "integer"
    },
    "path": {
      "description": "Path to the file to read",
      "type": "string"
    }
  },
  "required": [
    "path"
  ],
  "title": "FileReadConfig",
  "type": "object"
}
```

</details>
