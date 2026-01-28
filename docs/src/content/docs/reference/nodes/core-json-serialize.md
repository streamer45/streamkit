---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "core::json_serialize"
description: "Converts structured packets (Text, Transcription) to JSON-formatted text. Useful for logging, debugging, or sending data to external services."
---

`kind`: `core::json_serialize`

Converts structured packets (Text, Transcription) to JSON-formatted text. Useful for logging, debugging, or sending data to external services.

## Categories
- `core`
- `serialization`

## Pins
### Inputs
- `in` accepts `Any` (one)

### Outputs
- `out` produces `Binary` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `newline_delimited` | `boolean` | no | `false` | Add newline after each JSON object (for NDJSON format) |
| `pretty` | `boolean` | no | `false` | Enable pretty-printing (formatted with indentation) |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "JsonSerializeConfig",
  "description": "Configuration for JSON serialization",
  "type": "object",
  "properties": {
    "pretty": {
      "description": "Enable pretty-printing (formatted with indentation)",
      "type": "boolean",
      "default": false
    },
    "newline_delimited": {
      "description": "Add newline after each JSON object (for NDJSON format)",
      "type": "boolean",
      "default": false
    }
  }
}
```

</details>
