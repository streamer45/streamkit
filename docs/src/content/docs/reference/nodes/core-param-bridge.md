---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "core::param_bridge"
description: "Bridges data-plane packets to control-plane UpdateParams messages. Accepts any packet type and sends a mapped UpdateParams to a configured target node, enabling cross-node control within the pipeline graph. Supports auto, template, and raw mapping modes."
---

`kind`: `core::param_bridge`

Bridges data-plane packets to control-plane UpdateParams messages. Accepts any packet type and sends a mapped UpdateParams to a configured target node, enabling cross-node control within the pipeline graph. Supports auto, template, and raw mapping modes.

## Categories
- `core`
- `control`

## Pins
### Inputs
- `in` accepts `Any` (one)

### Outputs
No outputs.

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `debounce_ms` | `integer | null (uint64)` | no | `null` | Optional debounce window in milliseconds.<br /><br />When set, rapid `UpdateParams` messages are coalesced: only the most<br />recent value is sent after the window expires.  This is useful for<br />targets like subtitles where intermediate transcription segments are<br />superseded by newer ones.<br />min: `0` |
| `mode` | `string` | no | — | How the bridge maps incoming packets to `UpdateParams` JSON. |
| `target_node` | `string` | yes | — | The `node_id` of the sibling node to send `UpdateParams` to. |
| `template` | `value` | no | `null` | JSON template used when `mode` is `template`.<br /><br />Placeholders like `{{ text }}` (or `{{text}}`) are replaced with values<br />extracted from the incoming packet.<br /><br />Currently only `{{ text }}` is supported.  Future extensions could add<br />`{{ language }}`, `{{ confidence }}`, or arbitrary field paths. |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$defs": {
    "MappingMode": {
      "description": "How the bridge maps incoming packets to `UpdateParams` JSON.",
      "oneOf": [
        {
          "const": "auto",
          "description": "Smart per-packet-type mapping.\n\n`Transcription` and `Text` packets are wrapped in\n`{ \"properties\": { \"text\": \"...\" } }` — a shape that targets Slint\nplugin nodes out of the box.  `Custom` packets forward their `data`\nfield as-is (assumed to already be the correct `UpdateParams` shape).\n\nIf you need a different output shape (e.g. targeting a compositor's\n`text_overlays`), use `template` mode instead.",
          "type": "string"
        },
        {
          "const": "template",
          "description": "User-provided JSON template with `{{ text }}` placeholders.",
          "type": "string"
        },
        {
          "const": "raw",
          "description": "Forward the extracted payload as-is (no transformation).",
          "type": "string"
        }
      ]
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Configuration for the `core::param_bridge` node.",
  "properties": {
    "debounce_ms": {
      "default": null,
      "description": "Optional debounce window in milliseconds.\n\nWhen set, rapid `UpdateParams` messages are coalesced: only the most\nrecent value is sent after the window expires.  This is useful for\ntargets like subtitles where intermediate transcription segments are\nsuperseded by newer ones.",
      "format": "uint64",
      "minimum": 0,
      "type": [
        "integer",
        "null"
      ]
    },
    "mode": {
      "$ref": "#/$defs/MappingMode",
      "default": "auto",
      "description": "Mapping strategy."
    },
    "target_node": {
      "description": "The `node_id` of the sibling node to send `UpdateParams` to.",
      "type": "string"
    },
    "template": {
      "default": null,
      "description": "JSON template used when `mode` is `template`.\n\nPlaceholders like `{{ text }}` (or `{{text}}`) are replaced with values\nextracted from the incoming packet.\n\nCurrently only `{{ text }}` is supported.  Future extensions could add\n`{{ language }}`, `{{ confidence }}`, or arbitrary field paths."
    }
  },
  "required": [
    "target_node"
  ],
  "title": "ParamBridgeConfig",
  "type": "object"
}
```

</details>
