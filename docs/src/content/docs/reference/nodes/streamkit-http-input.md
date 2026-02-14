---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "streamkit::http_input"
description: "Synthetic input node for oneshot HTTP pipelines. Receives binary data from the HTTP request body."
---

`kind`: `streamkit::http_input`

Synthetic input node for oneshot HTTP pipelines. Receives binary data from the HTTP request body.

## Categories
- `transport`
- `oneshot`

## Pins
### Inputs
No inputs.

### Outputs
- `out` produces `Binary` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `field` | `string` | no | — | Multipart field name to bind to this input. Defaults to 'media' when only one http_input node exists; otherwise defaults to the node id. |
| `fields` | `array<object | string>` | no | — | Optional list of multipart fields for this node. When set, the node exposes one output pin per entry (pin name matches the field name). Entries may be strings or objects with { name, required }. |
| `required` | `boolean` | no | `true` | If true (default), the request must include this field. |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "type": "object",
  "additionalProperties": false,
  "properties": {
    "field": {
      "type": "string",
      "description": "Multipart field name to bind to this input. Defaults to 'media' when only one http_input node exists; otherwise defaults to the node id."
    },
    "fields": {
      "type": "array",
      "description": "Optional list of multipart fields for this node. When set, the node exposes one output pin per entry (pin name matches the field name). Entries may be strings or objects with { name, required }.",
      "items": {
        "oneOf": [
          {
            "type": "string"
          },
          {
            "type": "object",
            "additionalProperties": false,
            "properties": {
              "name": {
                "type": "string"
              },
              "required": {
                "type": "boolean",
                "default": true
              }
            },
            "required": [
              "name"
            ]
          }
        ]
      }
    },
    "required": {
      "type": "boolean",
      "description": "If true (default), the request must include this field.",
      "default": true
    }
  }
}
```

</details>
