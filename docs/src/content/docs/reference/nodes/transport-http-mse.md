---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "transport::http::mse"
description: "Serves WebM streams to HTTP clients for MSE (Media Source Extensions) playback. Accepts binary data from an upstream WebM muxer and broadcasts to multiple concurrent HTTP clients with init segment replay for late-joiners."
---

`kind`: `transport::http::mse`

Serves WebM streams to HTTP clients for MSE (Media Source Extensions) playback. Accepts binary data from an upstream WebM muxer and broadcasts to multiple concurrent HTTP clients with init segment replay for late-joiners.

## Categories
- `transport`
- `http`
- `mse`

## Pins
### Inputs
- `in` accepts `Binary` (one)

### Outputs
No outputs.

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `content_type` | `null | string` | no | `null` | Content type for the HTTP response.<br />Defaults to `video/webm; codecs="vp9,opus"`.<br />For best MSE compatibility, include the codecs parameter<br />(e.g., `video/webm; codecs="vp9,opus"` or `video/webm; codecs="vp9"`). |
| `max_clients` | `integer (uint32)` | no | `10` | Maximum concurrent HTTP clients (default: 10).<br />min: `1` |
| `path` | `string` | yes | — | Path suffix for the MSE stream endpoint (e.g., "/video").<br />Full URL will be: `/mse/{session_id}{path}` |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Configuration for the HttpMse node.",
  "properties": {
    "content_type": {
      "default": null,
      "description": "Content type for the HTTP response.\nDefaults to `video/webm; codecs=\"vp9,opus\"`.\nFor best MSE compatibility, include the codecs parameter\n(e.g., `video/webm; codecs=\"vp9,opus\"` or `video/webm; codecs=\"vp9\"`).",
      "type": [
        "string",
        "null"
      ]
    },
    "max_clients": {
      "default": 10,
      "description": "Maximum concurrent HTTP clients (default: 10).",
      "format": "uint32",
      "minimum": 1,
      "type": "integer"
    },
    "path": {
      "description": "Path suffix for the MSE stream endpoint (e.g., \"/video\").\nFull URL will be: `/mse/{session_id}{path}`",
      "type": "string"
    }
  },
  "required": [
    "path"
  ],
  "title": "HttpMseConfig",
  "type": "object"
}
```

</details>
