---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "core::file_writer"
description: "Writes incoming binary packets to a file. Security: the server validates write paths against `security.allowed_write_paths` (default deny)."
---

`kind`: `core::file_writer`

Writes incoming binary packets to a file. Security: the server validates write paths against `security.allowed_write_paths` (default deny).

## Categories
- `io`
- `file`

## Pins
### Inputs
- `in` accepts `Binary` (one)

### Outputs
No outputs.

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `chunk_size` | `integer (uint)` | no | `8192` | Size of buffer before writing to disk (default: 8192 bytes)<br />min: `0` |
| `path` | `string` | yes | — | Path to the file to write, relative to `[server].asset_root`.<br />Absolute paths and `..` components are rejected. |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "chunk_size": {
      "default": 8192,
      "description": "Size of buffer before writing to disk (default: 8192 bytes)",
      "format": "uint",
      "minimum": 0,
      "type": "integer"
    },
    "path": {
      "description": "Path to the file to write, relative to `[server].asset_root`.\nAbsolute paths and `..` components are rejected.",
      "type": "string"
    }
  },
  "required": [
    "path"
  ],
  "title": "FileWriteConfig",
  "type": "object"
}
```

</details>
