---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: "video::compositor"
description: "Composites multiple raw video inputs (RGBA8) onto a single canvas with image and text overlays. Supports dynamic pin creation for attaching arbitrary inputs at runtime."
---

`kind`: `video::compositor`

Composites multiple raw video inputs (RGBA8) onto a single canvas with image and text overlays. Supports dynamic pin creation for attaching arbitrary inputs at runtime.

## Categories
- `video`
- `compositing`

## Pins
### Inputs
- `in` accepts `RawVideo(RawVideoFormat { width: None, height: None, pixel_format: Rgba8 }), RawVideo(RawVideoFormat { width: None, height: None, pixel_format: I420 }), RawVideo(RawVideoFormat { width: None, height: None, pixel_format: Nv12 })` (dynamic)

### Outputs
- `out` produces `RawVideo(RawVideoFormat { width: None, height: None, pixel_format: Rgba8 })` (broadcast)

## Parameters
| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `fps` | `integer (uint32)` | no | `30` | Output frame rate.  The compositor ticks at this fixed rate<br />regardless of input frame rates, compositing with the latest<br />available frame from each input.<br />min: `0` |
| `height` | `integer (uint32)` | no | `720` | Output canvas height in pixels.<br />min: `0` |
| `image_overlays` | `array<object>` | no | — | Static image overlays (decoded once during init). |
| `layers` | `object` | no | — | Per-layer configuration, keyed by pin name (e.g. `"in_0"`).<br />Layers without an entry here are scaled to fill the canvas. |
| `num_inputs` | `integer | null (uint)` | no | `null` | Number of input pins to pre-create.<br />Required for stateless/oneshot pipelines where pins must exist before<br />graph building. Optional for dynamic pipelines where pins are created<br />on-demand. If specified, pins will be named in_0, in_1, ..., in_{N-1}.<br />min: `0` |
| `text_overlays` | `array<object>` | no | — | Text overlays (rasterized once per `UpdateParams`). |
| `width` | `integer (uint32)` | no | `1280` | Output canvas width in pixels.<br />min: `0` |

### `image_overlays` fields

| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `data_base64` | `string` | yes | — | Base64-encoded image data (PNG or JPEG). Decoded once during<br />initialization, not per-frame. |
| `id` | `string` | no | *(auto-generated UUID v4)* | Stable unique identifier.  Auto-generated (UUID v4) when omitted. |
| `mirror_horizontal` | `boolean` | no | `false` | Mirror the layer horizontally (flip left ↔ right).  Default `false`. |
| `mirror_vertical` | `boolean` | no | `false` | Mirror the layer vertically (flip top ↔ bottom).  Default `false`. |
| `opacity` | `number (float)` | no | `1.0` | Opacity multiplier (0.0 = fully transparent, 1.0 = fully opaque). |
| `rect` | `object` | yes | — | Pixel-space rectangle for positioning a layer on the output canvas.<br /><br />`x` and `y` are signed to allow off-screen positioning (e.g. for<br />slide-in effects or rotation around the rect centre). |
| `rotation_degrees` | `number (float)` | no | `0.0` | Clockwise rotation in degrees around the rect centre.  Default 0.0. |
| `z_index` | `integer (int32)` | no | `0` | Visual stacking order.  Lower values are drawn first (bottom);<br />higher values are drawn on top.  Default 0. |

#### `rect` fields

| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `height` | `integer (uint32)` | yes | — | min: `0` |
| `width` | `integer (uint32)` | yes | — | min: `0` |
| `x` | `integer (int32)` | yes | — | — |
| `y` | `integer (int32)` | yes | — | — |

### `layers` fields

| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `crop_x` | `number (float)` | no | `0.5` | Normalized horizontal pan position for the crop window<br />(0.0 = left edge, 0.5 = centred, 1.0 = right edge).  Only has a<br />visible effect when `crop_zoom > 1.0`.  Default 0.5. |
| `crop_y` | `number (float)` | no | `0.5` | Normalized vertical tilt position for the crop window<br />(0.0 = top edge, 0.5 = centred, 1.0 = bottom edge).  Only has a<br />visible effect when `crop_zoom > 1.0`.  Default 0.5. |
| `crop_zoom` | `number (float)` | no | `1.0` | Zoom factor for virtual PTZ crop (1.0 = full source, 2.0 = 2× zoom<br />showing the central 50% of the source).  Default 1.0. |
| `mirror_horizontal` | `boolean` | no | `false` | Mirror the layer horizontally (flip left ↔ right).  Default `false`. |
| `mirror_vertical` | `boolean` | no | `false` | Mirror the layer vertically (flip top ↔ bottom).  Default `false`. |
| `opacity` | `number (float)` | no | `1.0` | Opacity (0.0 .. 1.0). Default 1.0. |
| `rect` | `null | object` | no | — | Destination rectangle on the output canvas. If `None`, the input is<br />scaled to fill the entire canvas. |
| `rotation_degrees` | `number (float)` | no | `0.0` | Clockwise rotation in degrees.  Default 0.0 (no rotation).<br />The layer is rotated around its destination rect centre. |
| `z_index` | `integer (int32)` | no | `0` | Visual stacking order.  Lower values are drawn first (bottom);<br />higher values are drawn on top.  Ties are broken by slot index<br />(pin insertion order).  Default 0. |

### `text_overlays` fields

| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `color` | `array<integer (uint8)>` | no | `[255,255,255,255]` | RGBA colour, e.g. `[255, 255, 255, 255]`. |
| `font_name` | `null | string` | no | `null` | Font identifier: either a bundled font name or a font asset path.<br />Bundled names: "dejavu-sans", "dejavu-sans-bold",<br />"dejavu-sans-mono", "dejavu-sans-mono-bold",<br />"dejavu-serif", "dejavu-serif-bold".<br />Font asset paths (e.g. `"samples/fonts/system/Inter.ttf"`) are<br />managed via the `/api/v1/assets/fonts` REST API.<br />When omitted, the bundled default font (DejaVu Sans) is used. |
| `font_size` | `integer (uint32)` | no | `24` | Font size in pixels.<br />min: `0` |
| `id` | `string` | no | *(auto-generated UUID v4)* | Stable unique identifier.  Auto-generated (UUID v4) when omitted. |
| `mirror_horizontal` | `boolean` | no | `false` | Mirror the layer horizontally (flip left ↔ right).  Default `false`. |
| `mirror_vertical` | `boolean` | no | `false` | Mirror the layer vertically (flip top ↔ bottom).  Default `false`. |
| `opacity` | `number (float)` | no | `1.0` | Opacity multiplier (0.0 = fully transparent, 1.0 = fully opaque). |
| `rect` | `object` | yes | — | Pixel-space rectangle for positioning a layer on the output canvas.<br /><br />`x` and `y` are signed to allow off-screen positioning (e.g. for<br />slide-in effects or rotation around the rect centre). |
| `rotation_degrees` | `number (float)` | no | `0.0` | Clockwise rotation in degrees around the rect centre.  Default 0.0. |
| `text` | `string` | yes | — | The text string to render. |
| `z_index` | `integer (int32)` | no | `0` | Visual stacking order.  Lower values are drawn first (bottom);<br />higher values are drawn on top.  Default 0. |

#### `rect` fields

| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `height` | `integer (uint32)` | yes | — | min: `0` |
| `width` | `integer (uint32)` | yes | — | min: `0` |
| `x` | `integer (int32)` | yes | — | — |
| `y` | `integer (int32)` | yes | — | — |


<details>
<summary>Raw JSON Schema</summary>

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "CompositorConfig",
  "description": "Configuration for the video compositor node.\n\nThe compositor supports an arbitrary number of dynamic video inputs\n(created at runtime via `PinManagementMessage`) plus static image/text\noverlays configured here.",
  "type": "object",
  "properties": {
    "width": {
      "description": "Output canvas width in pixels.",
      "type": "integer",
      "format": "uint32",
      "minimum": 0,
      "default": 1280
    },
    "height": {
      "description": "Output canvas height in pixels.",
      "type": "integer",
      "format": "uint32",
      "minimum": 0,
      "default": 720
    },
    "fps": {
      "description": "Output frame rate.  The compositor ticks at this fixed rate\nregardless of input frame rates, compositing with the latest\navailable frame from each input.",
      "type": "integer",
      "format": "uint32",
      "minimum": 0,
      "default": 30
    },
    "num_inputs": {
      "description": "Number of input pins to pre-create.\nRequired for stateless/oneshot pipelines where pins must exist before\ngraph building. Optional for dynamic pipelines where pins are created\non-demand. If specified, pins will be named in_0, in_1, ..., in_{N-1}.",
      "type": [
        "integer",
        "null"
      ],
      "format": "uint",
      "minimum": 0,
      "default": null
    },
    "layers": {
      "description": "Per-layer configuration, keyed by pin name (e.g. `\"in_0\"`).\nLayers without an entry here are scaled to fill the canvas.",
      "type": "object",
      "additionalProperties": {
        "$ref": "#/$defs/LayerConfig"
      }
    },
    "image_overlays": {
      "description": "Static image overlays (decoded once during init).",
      "type": "array",
      "items": {
        "$ref": "#/$defs/ImageOverlayConfig"
      }
    },
    "text_overlays": {
      "description": "Text overlays (rasterized once per `UpdateParams`).",
      "type": "array",
      "items": {
        "$ref": "#/$defs/TextOverlayConfig"
      }
    }
  },
  "$defs": {
    "LayerConfig": {
      "description": "Layer configuration for a single compositing input.",
      "type": "object",
      "properties": {
        "rect": {
          "description": "Destination rectangle on the output canvas. If `None`, the input is\nscaled to fill the entire canvas.",
          "anyOf": [
            {
              "$ref": "#/$defs/Rect"
            },
            {
              "type": "null"
            }
          ]
        },
        "opacity": {
          "description": "Opacity (0.0 .. 1.0). Default 1.0.",
          "type": "number",
          "format": "float",
          "default": 1.0
        },
        "z_index": {
          "description": "Visual stacking order.  Lower values are drawn first (bottom);\nhigher values are drawn on top.  Ties are broken by slot index\n(pin insertion order).  Default 0.",
          "type": "integer",
          "format": "int32",
          "default": 0
        },
        "rotation_degrees": {
          "description": "Clockwise rotation in degrees.  Default 0.0 (no rotation).\nThe layer is rotated around its destination rect centre.",
          "type": "number",
          "format": "float",
          "default": 0.0
        },
        "mirror_horizontal": {
          "description": "Mirror the layer horizontally (flip left ↔ right).  Default `false`.",
          "type": "boolean",
          "default": false
        },
        "mirror_vertical": {
          "description": "Mirror the layer vertically (flip top ↔ bottom).  Default `false`.",
          "type": "boolean",
          "default": false
        },
        "crop_zoom": {
          "description": "Zoom factor for virtual PTZ crop (1.0 = full source, 2.0 = 2× zoom\nshowing the central 50% of the source).  Default 1.0.",
          "type": "number",
          "format": "float",
          "default": 1.0
        },
        "crop_x": {
          "description": "Normalized horizontal pan position for the crop window\n(0.0 = left edge, 0.5 = centred, 1.0 = right edge).  Only has a\nvisible effect when `crop_zoom > 1.0`.  Default 0.5.",
          "type": "number",
          "format": "float",
          "default": 0.5
        },
        "crop_y": {
          "description": "Normalized vertical tilt position for the crop window\n(0.0 = top edge, 0.5 = centred, 1.0 = bottom edge).  Only has a\nvisible effect when `crop_zoom > 1.0`.  Default 0.5.",
          "type": "number",
          "format": "float",
          "default": 0.5
        }
      }
    },
    "Rect": {
      "description": "Pixel-space rectangle for positioning a layer on the output canvas.\n\n`x` and `y` are signed to allow off-screen positioning (e.g. for\nslide-in effects or rotation around the rect centre).",
      "type": "object",
      "properties": {
        "x": {
          "type": "integer",
          "format": "int32"
        },
        "y": {
          "type": "integer",
          "format": "int32"
        },
        "width": {
          "type": "integer",
          "format": "uint32",
          "minimum": 0
        },
        "height": {
          "type": "integer",
          "format": "uint32",
          "minimum": 0
        }
      },
      "required": [
        "x",
        "y",
        "width",
        "height"
      ]
    },
    "ImageOverlayConfig": {
      "description": "Configuration for a static image overlay (decoded once at init).",
      "type": "object",
      "properties": {
        "id": {
          "description": "Stable unique identifier.  Auto-generated (UUID v4) when omitted.",
          "type": "string",
          "default": "(auto-generated UUID v4)"
        },
        "data_base64": {
          "description": "Base64-encoded image data (PNG or JPEG). Decoded once during\ninitialization, not per-frame.",
          "type": "string"
        },
        "rect": {
          "description": "Destination rectangle on the output canvas.",
          "$ref": "#/$defs/Rect"
        },
        "opacity": {
          "description": "Opacity multiplier (0.0 = fully transparent, 1.0 = fully opaque).",
          "type": "number",
          "format": "float",
          "default": 1.0
        },
        "rotation_degrees": {
          "description": "Clockwise rotation in degrees around the rect centre.  Default 0.0.",
          "type": "number",
          "format": "float",
          "default": 0.0
        },
        "z_index": {
          "description": "Visual stacking order.  Lower values are drawn first (bottom);\nhigher values are drawn on top.  Default 0.",
          "type": "integer",
          "format": "int32",
          "default": 0
        },
        "mirror_horizontal": {
          "description": "Mirror the layer horizontally (flip left ↔ right).  Default `false`.",
          "type": "boolean",
          "default": false
        },
        "mirror_vertical": {
          "description": "Mirror the layer vertically (flip top ↔ bottom).  Default `false`.",
          "type": "boolean",
          "default": false
        }
      },
      "required": [
        "data_base64",
        "rect"
      ]
    },
    "TextOverlayConfig": {
      "description": "Configuration for a text overlay (rasterized once per `UpdateParams`).",
      "type": "object",
      "properties": {
        "id": {
          "description": "Stable unique identifier.  Auto-generated (UUID v4) when omitted.",
          "type": "string",
          "default": "(auto-generated UUID v4)"
        },
        "text": {
          "description": "The text string to render.",
          "type": "string"
        },
        "rect": {
          "description": "Destination rectangle on the output canvas.",
          "$ref": "#/$defs/Rect"
        },
        "opacity": {
          "description": "Opacity multiplier (0.0 = fully transparent, 1.0 = fully opaque).",
          "type": "number",
          "format": "float",
          "default": 1.0
        },
        "rotation_degrees": {
          "description": "Clockwise rotation in degrees around the rect centre.  Default 0.0.",
          "type": "number",
          "format": "float",
          "default": 0.0
        },
        "z_index": {
          "description": "Visual stacking order.  Lower values are drawn first (bottom);\nhigher values are drawn on top.  Default 0.",
          "type": "integer",
          "format": "int32",
          "default": 0
        },
        "mirror_horizontal": {
          "description": "Mirror the layer horizontally (flip left ↔ right).  Default `false`.",
          "type": "boolean",
          "default": false
        },
        "mirror_vertical": {
          "description": "Mirror the layer vertically (flip top ↔ bottom).  Default `false`.",
          "type": "boolean",
          "default": false
        },
        "color": {
          "description": "RGBA colour, e.g. `[255, 255, 255, 255]`.",
          "type": "array",
          "items": {
            "type": "integer",
            "format": "uint8",
            "minimum": 0,
            "maximum": 255
          },
          "minItems": 4,
          "maxItems": 4,
          "default": [
            255,
            255,
            255,
            255
          ]
        },
        "font_size": {
          "description": "Font size in pixels.",
          "type": "integer",
          "format": "uint32",
          "minimum": 0,
          "default": 24
        },
        "font_name": {
          "description": "Font identifier: either a bundled font name (e.g. \"dejavu-sans\") or a font asset path (e.g. \"samples/fonts/system/Inter.ttf\").\nBundled names: \"dejavu-sans\", \"dejavu-sans-bold\",\n\"dejavu-sans-mono\", \"dejavu-sans-mono-bold\",\n\"dejavu-serif\", \"dejavu-serif-bold\".\nFont asset paths are managed via the /api/v1/assets/fonts REST API.\nWhen omitted, the bundled default font (DejaVu Sans) is used.",
          "type": [
            "string",
            "null"
          ],
          "default": null
        }
      },
      "required": [
        "text",
        "rect"
      ]
    }
  }
}
```

</details>
