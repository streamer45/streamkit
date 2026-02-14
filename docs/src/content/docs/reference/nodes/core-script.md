---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "core::script"
description: "Execute custom JavaScript code for API integration, webhooks, text transformation, and dynamic routing. Provides a sandboxed QuickJS runtime with fetch() API support. See the [Script Node Guide](/guides/script-node/) for detailed usage."
---

`kind`: `core::script`

Execute custom JavaScript code for API integration, webhooks, text transformation, and dynamic routing. Provides a sandboxed QuickJS runtime with fetch() API support. See the [Script Node Guide](/guides/script-node/) for detailed usage.

## Categories
- `core`
- `scripting`

## Pins
### Inputs
- `in` accepts `Any` (one)

### Outputs
- `out` produces `Passthrough` (one)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `headers` | `array<object>` | no | `[]` | Header mappings for fetch() calls<br />Maps secret names to HTTP headers with optional templates |
| `memory_limit_mb` | `integer (uint)` | no | `64` | QuickJS memory limit in MB (default: 64MB)<br />min: `0` |
| `script` | `string` | no | — | JavaScript code (must define a process(packet) function) |
| `script_path` | `null | string` | no | `null` | Optional path to a JavaScript file to load as the script.<br /><br />If set, the file contents are loaded at node creation time.<br />For security, the StreamKit server validates this path against `security.allowed_file_paths`. |
| `timeout_ms` | `integer (uint64)` | no | `100` | Per-packet timeout in milliseconds (default: 100ms)<br />min: `0` |

### `headers` fields

| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `header` | `string` | yes | — | HTTP header name (e.g., "Authorization", "X-API-Key") |
| `secret` | `string` | yes | — | Secret name (must exist in server config's [script.secrets]) |
| `template` | `string` | no | `{}` | Optional template for formatting the header value<br />Use {} as placeholder for the secret value<br />Examples: "Bearer {}", "token {}", "ApiKey {}"<br />Default: "{}" (raw value) |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "ScriptConfig",
  "description": "Configuration for the script node",
  "type": "object",
  "properties": {
    "script": {
      "description": "JavaScript code (must define a process(packet) function)",
      "type": "string",
      "default": ""
    },
    "script_path": {
      "description": "Optional path to a JavaScript file to load as the script.\n\nIf set, the file contents are loaded at node creation time.\nFor security, the StreamKit server validates this path against `security.allowed_file_paths`.",
      "type": [
        "string",
        "null"
      ],
      "default": null
    },
    "timeout_ms": {
      "description": "Per-packet timeout in milliseconds (default: 100ms)",
      "type": "integer",
      "format": "uint64",
      "minimum": 0,
      "default": 100
    },
    "memory_limit_mb": {
      "description": "QuickJS memory limit in MB (default: 64MB)",
      "type": "integer",
      "format": "uint",
      "minimum": 0,
      "default": 64
    },
    "headers": {
      "description": "Header mappings for fetch() calls\nMaps secret names to HTTP headers with optional templates",
      "type": "array",
      "items": {
        "$ref": "#/$defs/HeaderMapping"
      },
      "default": []
    }
  },
  "$defs": {
    "HeaderMapping": {
      "description": "Maps a server-configured secret to an HTTP header for fetch() calls",
      "type": "object",
      "properties": {
        "secret": {
          "description": "Secret name (must exist in server config's [script.secrets])",
          "type": "string"
        },
        "header": {
          "description": "HTTP header name (e.g., \"Authorization\", \"X-API-Key\")",
          "type": "string"
        },
        "template": {
          "description": "Optional template for formatting the header value\nUse {} as placeholder for the secret value\nExamples: \"Bearer {}\", \"token {}\", \"ApiKey {}\"\nDefault: \"{}\" (raw value)",
          "type": "string",
          "default": "{}"
        }
      },
      "required": [
        "secret",
        "header"
      ]
    }
  }
}
```

</details>
