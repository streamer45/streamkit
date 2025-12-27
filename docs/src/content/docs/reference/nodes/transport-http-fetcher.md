---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "transport::http::fetcher"
description: "Fetches binary data from an HTTP/HTTPS URL. Security: this is an SSRF-capable node; restrict it via role allowlists. Redirects are disabled (v0.1.0)."
---

`kind`: `transport::http::fetcher`

Fetches binary data from an HTTP/HTTPS URL. Security: this is an SSRF-capable node; restrict it via role allowlists. Redirects are disabled (v0.1.0).

## Categories
- `transport`
- `http`

## Pins
### Inputs
No inputs.

### Outputs
- `out` produces `Binary` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `chunk_size` | `integer (uint)` | no | `8192` | Size of chunks to read (default: 8192 bytes)<br />min: `1` |
| `url` | `string` | yes | — | URL to fetch (HTTP or HTTPS) |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "description": "Configuration for the HttpPullNode",
  "properties": {
    "chunk_size": {
      "default": 8192,
      "description": "Size of chunks to read (default: 8192 bytes)",
      "format": "uint",
      "minimum": 1,
      "type": "integer"
    },
    "url": {
      "description": "URL to fetch (HTTP or HTTPS)",
      "type": "string"
    }
  },
  "required": [
    "url"
  ],
  "title": "HttpPullConfig",
  "type": "object"
}
```

</details>
