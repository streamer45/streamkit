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
| `gpu_mode` | `null | string` | no | `null` | GPU compositing preference.  Default `None` (treated as `"auto"`).<br />- `"auto"` (default): probe for GPU at startup; use it when scene<br />  complexity warrants (multi-layer, high-res, effects)<br />- `"gpu"`: force GPU compositing for every frame (warn and fall back<br />  to CPU if unavailable)<br />- `"cpu"`: force CPU compositing (ignore GPU even if available)<br /><br />When unset or `"auto"`, the compositor initialises the GPU at startup<br />and uses a per-frame heuristic to decide whether each frame benefits<br />from GPU acceleration.  Simple single-layer scenes use the faster CPU<br />memcpy path.  Set to `"cpu"` to explicitly disable GPU acceleration. |
| `height` | `integer (uint32)` | no | `720` | Output canvas height in pixels.<br />min: `0` |
| `image_overlays` | `array<object>` | no | — | Static image overlays (decoded once during init). |
| `layers` | `object` | no | — | Per-layer configuration, keyed by pin name (e.g. `"in_0"`).<br />Layers without an entry here are scaled to fill the canvas. |
| `num_inputs` | `integer | null (uint)` | no | `null` | Number of input pins to pre-create.<br />Required for stateless/oneshot pipelines where pins must exist before<br />graph building. Optional for dynamic pipelines where pins are created<br />on-demand. If specified, pins will be named in_0, in_1, ..., in_{N-1}.<br />min: `0` |
| `output_format` | `null | string` | no | `null` | Optional output pixel format conversion.  When set to `"nv12"` or<br />`"i420"`, the compositor converts its RGBA8 canvas to the target<br />format on the compositing thread while data is still cache-hot.<br />Default: `None` (output RGBA8). |
| `text_overlays` | `array<object>` | no | — | Text overlays (rasterized once per `UpdateParams`). |
| `width` | `integer (uint32)` | no | `1280` | Output canvas width in pixels.<br />min: `0` |

### `image_overlays` fields

| Name | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `asset_path` | `string` | yes | — | Server-relative path to an uploaded image asset<br />(e.g. `samples/images/user/logo.png`). |
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
| `aspect_fit` | `boolean` | no | `true` | When `true` (the default), the source is fitted within the<br />destination rect while preserving its native aspect ratio<br />(letterbox / pillarbox).  Set to `false` to stretch the source<br />to fill the rect exactly. |
| `crop_shape` | `string` | no | — | Shape used to clip a composited layer.<br /><br />`Rect` (the default) renders the layer as-is within its destination<br />rectangle.  `Circle` clips to an ellipse inscribed in the destination<br />rect — when the rect is square this produces a perfect circle, ideal<br />for Loom-style webcam PIP overlays.<br /><br />New variants (e.g. `RoundedRect`, `Hexagon`) can be added in the<br />future.  The field-level `#[serde(default)]` on `LayerConfig` means a<br />missing `crop_shape` key defaults to `Rect`. |
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
| `font_name` | `null | string` | no | `null` | Font identifier: a font asset path under `samples/fonts/`, e.g.<br />`"samples/fonts/system/DejaVuSans.ttf"` or<br />`"samples/fonts/system/Inter.ttf"`.<br /><br />Font assets are TTF/OTF files managed via the `/api/v1/assets/fonts`<br />REST API and stored under `samples/fonts/{system,user}/`.<br /><br />When omitted, the default system font (DejaVu Sans) is used. |
| `font_size` | `integer (uint32)` | no | `24` | Font size in pixels.<br />min: `0` |
| `id` | `string` | no | *(auto-generated UUID v4)* | Stable unique identifier.  Auto-generated (UUID v4) when omitted. |
| `mirror_horizontal` | `boolean` | no | `false` | Mirror the layer horizontally (flip left ↔ right).  Default `false`. |
| `mirror_vertical` | `boolean` | no | `false` | Mirror the layer vertically (flip top ↔ bottom).  Default `false`. |
| `opacity` | `number (float)` | no | `1.0` | Opacity multiplier (0.0 = fully transparent, 1.0 = fully opaque). |
| `rect` | `object` | yes | — | Pixel-space rectangle for positioning a layer on the output canvas.<br /><br />`x` and `y` are signed to allow off-screen positioning (e.g. for<br />slide-in effects or rotation around the rect centre). |
| `rotation_degrees` | `number (float)` | no | `0.0` | Clockwise rotation in degrees around the rect centre.  Default 0.0. |
| `text` | `string` | yes | — | The text string to render. |
| `word_wrap` | `boolean` | no | `false` | Enable word wrapping within the overlay's bounding rectangle.<br /><br />When `true`, text is wrapped at the width specified by<br />`transform.rect.width`.  When `false` (the default), text only<br />breaks on explicit newlines — matching the historical behaviour. |
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
  "$defs": {
    "CropShape": {
      "description": "Shape used to clip a composited layer.\n\n`Rect` (the default) renders the layer as-is within its destination\nrectangle.  `Circle` clips to an ellipse inscribed in the destination\nrect — when the rect is square this produces a perfect circle, ideal\nfor Loom-style webcam PIP overlays.\n\nNew variants (e.g. `RoundedRect`, `Hexagon`) can be added in the\nfuture.  The field-level `#[serde(default)]` on `LayerConfig` means a\nmissing `crop_shape` key defaults to `Rect`.",
      "oneOf": [
        {
          "const": "rect",
          "description": "No shape clipping — the layer fills its destination rectangle.",
          "type": "string"
        },
        {
          "const": "circle",
          "description": "Clip to a circle inscribed in the shorter side of the destination\nrectangle.  The circle is always a true circle (never an ellipse),\ncentred within the rect.",
          "type": "string"
        }
      ]
    },
    "ImageOverlayConfig": {
      "description": "Configuration for a static image overlay (decoded once at init).\n\nNote: `deny_unknown_fields` is intentionally omitted here because\n`#[serde(flatten)]` on `transform` is incompatible with it — serde\ncannot distinguish \"unknown\" fields from flattened struct fields.",
      "properties": {
        "asset_path": {
          "description": "Server-relative path to an uploaded image asset\n(e.g. `samples/images/user/logo.png`).",
          "type": "string"
        },
        "id": {
          "default": "(auto-generated UUID v4)",
          "description": "Stable unique identifier.  Auto-generated (UUID v4) when omitted.",
          "type": "string"
        },
        "mirror_horizontal": {
          "default": false,
          "description": "Mirror the layer horizontally (flip left ↔ right).  Default `false`.",
          "type": "boolean"
        },
        "mirror_vertical": {
          "default": false,
          "description": "Mirror the layer vertically (flip top ↔ bottom).  Default `false`.",
          "type": "boolean"
        },
        "opacity": {
          "default": 1.0,
          "description": "Opacity multiplier (0.0 = fully transparent, 1.0 = fully opaque).",
          "format": "float",
          "type": "number"
        },
        "rect": {
          "$ref": "#/$defs/Rect",
          "description": "Destination rectangle on the output canvas."
        },
        "rotation_degrees": {
          "default": 0.0,
          "description": "Clockwise rotation in degrees around the rect centre.  Default 0.0.",
          "format": "float",
          "type": "number"
        },
        "z_index": {
          "default": 0,
          "description": "Visual stacking order.  Lower values are drawn first (bottom);\nhigher values are drawn on top.  Default 0.",
          "format": "int32",
          "type": "integer"
        }
      },
      "required": [
        "asset_path",
        "rect"
      ],
      "type": "object"
    },
    "LayerConfig": {
      "additionalProperties": false,
      "description": "Layer configuration for a single compositing input.",
      "properties": {
        "aspect_fit": {
          "default": true,
          "description": "When `true` (the default), the source is fitted within the\ndestination rect while preserving its native aspect ratio\n(letterbox / pillarbox).  Set to `false` to stretch the source\nto fill the rect exactly.",
          "type": "boolean"
        },
        "crop_shape": {
          "$ref": "#/$defs/CropShape",
          "default": "rect",
          "description": "Shape clipping applied to the layer.  Default `Rect` (no clipping).\nSet to `Circle` for Loom-style circular webcam PIP overlays."
        },
        "crop_x": {
          "default": 0.5,
          "description": "Normalized horizontal pan position for the crop window\n(0.0 = left edge, 0.5 = centred, 1.0 = right edge).  Only has a\nvisible effect when `crop_zoom > 1.0`.  Default 0.5.",
          "format": "float",
          "type": "number"
        },
        "crop_y": {
          "default": 0.5,
          "description": "Normalized vertical tilt position for the crop window\n(0.0 = top edge, 0.5 = centred, 1.0 = bottom edge).  Only has a\nvisible effect when `crop_zoom > 1.0`.  Default 0.5.",
          "format": "float",
          "type": "number"
        },
        "crop_zoom": {
          "default": 1.0,
          "description": "Zoom factor for virtual PTZ crop (1.0 = full source, 2.0 = 2× zoom\nshowing the central 50% of the source).  Default 1.0.",
          "format": "float",
          "type": "number"
        },
        "mirror_horizontal": {
          "default": false,
          "description": "Mirror the layer horizontally (flip left ↔ right).  Default `false`.",
          "type": "boolean"
        },
        "mirror_vertical": {
          "default": false,
          "description": "Mirror the layer vertically (flip top ↔ bottom).  Default `false`.",
          "type": "boolean"
        },
        "opacity": {
          "default": 1.0,
          "description": "Opacity (0.0 .. 1.0). Default 1.0.",
          "format": "float",
          "type": "number"
        },
        "rect": {
          "anyOf": [
            {
              "$ref": "#/$defs/Rect"
            },
            {
              "type": "null"
            }
          ],
          "description": "Destination rectangle on the output canvas. If `None`, the input is\nscaled to fill the entire canvas."
        },
        "rotation_degrees": {
          "default": 0.0,
          "description": "Clockwise rotation in degrees.  Default 0.0 (no rotation).\nThe layer is rotated around its destination rect centre.",
          "format": "float",
          "type": "number"
        },
        "z_index": {
          "default": 0,
          "description": "Visual stacking order.  Lower values are drawn first (bottom);\nhigher values are drawn on top.  Ties are broken by slot index\n(pin insertion order).  Default 0.",
          "format": "int32",
          "type": "integer"
        }
      },
      "type": "object"
    },
    "Rect": {
      "description": "Pixel-space rectangle for positioning a layer on the output canvas.\n\n`x` and `y` are signed to allow off-screen positioning (e.g. for\nslide-in effects or rotation around the rect centre).",
      "properties": {
        "height": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        },
        "width": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        },
        "x": {
          "format": "int32",
          "type": "integer"
        },
        "y": {
          "format": "int32",
          "type": "integer"
        }
      },
      "required": [
        "x",
        "y",
        "width",
        "height"
      ],
      "type": "object"
    },
    "TextOverlayConfig": {
      "description": "Configuration for a text overlay (rasterized once per `UpdateParams`).\n\nNote: `deny_unknown_fields` is intentionally omitted here because\n`#[serde(flatten)]` on `transform` is incompatible with it — serde\ncannot distinguish \"unknown\" fields from flattened struct fields.",
      "properties": {
        "color": {
          "default": [
            255,
            255,
            255,
            255
          ],
          "description": "RGBA colour, e.g. `[255, 255, 255, 255]`.",
          "items": {
            "format": "uint8",
            "maximum": 255,
            "minimum": 0,
            "type": "integer"
          },
          "maxItems": 4,
          "minItems": 4,
          "type": "array"
        },
        "font_name": {
          "default": null,
          "description": "Font identifier: a font asset path under `samples/fonts/`, e.g.\n`\"samples/fonts/system/DejaVuSans.ttf\"` or\n`\"samples/fonts/system/Inter.ttf\"`.\n\nFont assets are TTF/OTF files managed via the `/api/v1/assets/fonts`\nREST API and stored under `samples/fonts/{system,user}/`.\n\nWhen omitted, the default system font (DejaVu Sans) is used.",
          "type": [
            "string",
            "null"
          ]
        },
        "font_size": {
          "default": 24,
          "description": "Font size in pixels.",
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        },
        "id": {
          "default": "(auto-generated UUID v4)",
          "description": "Stable unique identifier.  Auto-generated (UUID v4) when omitted.",
          "type": "string"
        },
        "mirror_horizontal": {
          "default": false,
          "description": "Mirror the layer horizontally (flip left ↔ right).  Default `false`.",
          "type": "boolean"
        },
        "mirror_vertical": {
          "default": false,
          "description": "Mirror the layer vertically (flip top ↔ bottom).  Default `false`.",
          "type": "boolean"
        },
        "opacity": {
          "default": 1.0,
          "description": "Opacity multiplier (0.0 = fully transparent, 1.0 = fully opaque).",
          "format": "float",
          "type": "number"
        },
        "rect": {
          "$ref": "#/$defs/Rect",
          "description": "Destination rectangle on the output canvas."
        },
        "rotation_degrees": {
          "default": 0.0,
          "description": "Clockwise rotation in degrees around the rect centre.  Default 0.0.",
          "format": "float",
          "type": "number"
        },
        "text": {
          "description": "The text string to render.",
          "type": "string"
        },
        "word_wrap": {
          "default": false,
          "description": "Enable word wrapping within the overlay's bounding rectangle.\n\nWhen `true`, text is wrapped at the width specified by\n`transform.rect.width`.  When `false` (the default), text only\nbreaks on explicit newlines — matching the historical behaviour.",
          "type": "boolean"
        },
        "z_index": {
          "default": 0,
          "description": "Visual stacking order.  Lower values are drawn first (bottom);\nhigher values are drawn on top.  Default 0.",
          "format": "int32",
          "type": "integer"
        }
      },
      "required": [
        "text",
        "rect"
      ],
      "type": "object"
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "description": "Configuration for the video compositor node.\n\nThe compositor supports an arbitrary number of dynamic video inputs\n(created at runtime via `PinManagementMessage`) plus static image/text\noverlays configured here.",
  "properties": {
    "fps": {
      "default": 30,
      "description": "Output frame rate.  The compositor ticks at this fixed rate\nregardless of input frame rates, compositing with the latest\navailable frame from each input.",
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    },
    "gpu_mode": {
      "default": null,
      "description": "GPU compositing preference.  Default `None` (treated as `\"auto\"`).\n- `\"auto\"` (default): probe for GPU at startup; use it when scene\n  complexity warrants (multi-layer, high-res, effects)\n- `\"gpu\"`: force GPU compositing for every frame (warn and fall back\n  to CPU if unavailable)\n- `\"cpu\"`: force CPU compositing (ignore GPU even if available)\n\nWhen unset or `\"auto\"`, the compositor initialises the GPU at startup\nand uses a per-frame heuristic to decide whether each frame benefits\nfrom GPU acceleration.  Simple single-layer scenes use the faster CPU\nmemcpy path.  Set to `\"cpu\"` to explicitly disable GPU acceleration.",
      "type": [
        "string",
        "null"
      ]
    },
    "height": {
      "default": 720,
      "description": "Output canvas height in pixels.",
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    },
    "image_overlays": {
      "description": "Static image overlays (decoded once during init).",
      "items": {
        "$ref": "#/$defs/ImageOverlayConfig"
      },
      "type": "array"
    },
    "layers": {
      "additionalProperties": {
        "$ref": "#/$defs/LayerConfig"
      },
      "description": "Per-layer configuration, keyed by pin name (e.g. `\"in_0\"`).\nLayers without an entry here are scaled to fill the canvas.",
      "type": "object"
    },
    "num_inputs": {
      "default": null,
      "description": "Number of input pins to pre-create.\nRequired for stateless/oneshot pipelines where pins must exist before\ngraph building. Optional for dynamic pipelines where pins are created\non-demand. If specified, pins will be named in_0, in_1, ..., in_{N-1}.",
      "format": "uint",
      "minimum": 0,
      "type": [
        "integer",
        "null"
      ]
    },
    "output_format": {
      "default": null,
      "description": "Optional output pixel format conversion.  When set to `\"nv12\"` or\n`\"i420\"`, the compositor converts its RGBA8 canvas to the target\nformat on the compositing thread while data is still cache-hot.\nDefault: `None` (output RGBA8).",
      "type": [
        "string",
        "null"
      ]
    },
    "text_overlays": {
      "description": "Text overlays (rasterized once per `UpdateParams`).",
      "items": {
        "$ref": "#/$defs/TextOverlayConfig"
      },
      "type": "array"
    },
    "width": {
      "default": 1280,
      "description": "Output canvas width in pixels.",
      "format": "uint32",
      "minimum": 0,
      "type": "integer"
    }
  },
  "title": "CompositorConfig",
  "type": "object"
}
```

</details>
