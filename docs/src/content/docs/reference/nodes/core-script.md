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
  "$defs": {
    "HeaderMapping": {
      "description": "Maps a server-configured secret to an HTTP header for fetch() calls",
      "properties": {
        "header": {
          "description": "HTTP header name (e.g., \"Authorization\", \"X-API-Key\")",
          "type": "string"
        },
        "secret": {
          "description": "Secret name (must exist in server config's [script.secrets])",
          "type": "string"
        },
        "template": {
          "default": "{}",
          "description": "Optional template for formatting the header value\nUse {} as placeholder for the secret value\nExamples: \"Bearer {}\", \"token {}\", \"ApiKey {}\"\nDefault: \"{}\" (raw value)",
          "type": "string"
        }
      },
      "required": [
        "secret",
        "header"
      ],
      "type": "object"
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "description": "Configuration for the script node",
  "properties": {
    "headers": {
      "default": [],
      "description": "Header mappings for fetch() calls\nMaps secret names to HTTP headers with optional templates",
      "items": {
        "$ref": "#/$defs/HeaderMapping"
      },
      "type": "array"
    },
    "memory_limit_mb": {
      "default": 64,
      "description": "QuickJS memory limit in MB (default: 64MB)",
      "format": "uint",
      "minimum": 0,
      "type": "integer"
    },
    "script": {
      "default": "",
      "description": "JavaScript code (must define a process(packet) function)",
      "type": "string"
    },
    "script_path": {
      "default": null,
      "description": "Optional path to a JavaScript file to load as the script.\n\nIf set, the file contents are loaded at node creation time.\nFor security, the StreamKit server validates this path against `security.allowed_file_paths`.",
      "type": [
        "string",
        "null"
      ]
    },
    "timeout_ms": {
      "default": 100,
      "description": "Per-packet timeout in milliseconds (default: 100ms)",
      "format": "uint64",
      "minimum": 0,
      "type": "integer"
    }
  },
  "title": "ScriptConfig",
  "type": "object"
}
```

</details>
