<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Slint Native Plugin

Render `.slint` UI files as a video source node. Produces RGBA8 frames at a
configurable resolution and frame rate using the Slint software renderer.

## Features

- **Declarative UI overlays** — design overlays in the Slint markup language
- **Software rendering** — no GPU required, runs anywhere
- **Runtime property updates** — change text, scores, colors via `UpdateParams`
- **Keyframe cycling** — animate through property snapshots over time
- **Static UI caching** — skip re-renders when properties haven't changed

## Setup

### Build Plugin

```bash
just build-plugin-native-slint
```

### Upload to Server

```bash
just upload-slint-plugin
```

Or manually:
```bash
curl -X POST \
  -F plugin=@target/plugins/release/libslint.so \
  http://127.0.0.1:4545/api/v1/plugins
```

### Verify Loaded

```bash
curl http://localhost:4545/api/v1/plugins
# Should show: plugin::native::slint
```

## Usage

### Example Pipeline

See `samples/pipelines/oneshot/video_slint_watermark.yml` for a static
watermark overlay example.

### Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `width` | integer | 640 | Output frame width in pixels |
| `height` | integer | 480 | Output frame height in pixels |
| `fps` | integer | 30 | Output frame rate |
| `slint_file` | string | *required* | Path to `.slint` file |
| `component` | string | *(first)* | Exported component name to instantiate |
| `properties` | object | `{}` | Key-value map of Slint properties |
| `property_keyframes` | array | `[]` | List of property snapshots to cycle through |
| `keyframe_interval` | integer | 90 | Frames between keyframe switches |
| `frame_count` | integer | 0 | Total frames to generate (0 = infinite) |
| `static_ui` | boolean | false | Cache frames when properties haven't changed |

### Property Types

Properties are mapped from JSON to Slint values:
- JSON strings → `SharedString`
- JSON numbers → `f64`
- JSON booleans → `bool`

### Static vs Dynamic UI

- **`static_ui: false`** (default): Every frame is re-rendered. Use for UIs
  with Slint `Timer` or `animate` directives.
- **`static_ui: true`**: Frames are cached and reused until properties change.
  Use for static overlays (watermarks, scoreboards updated only via
  `UpdateParams`).

## Architecture

### Threading Model

Slint types are `!Send` (Rc-based) and `slint::platform::set_platform` is
process-global. All Slint operations are funnelled through a single dedicated
`std::thread` (lazily spawned). Each plugin instance communicates with this
shared thread via tagged work items and per-instance result channels.

### Data Flow

```
Host tick loop → tick() → [Render work item] → Slint thread → render → [Frame result] → tick() → output.send()
```

## Technical Details

### Dependencies

- **slint** (1.15+): Slint runtime with software renderer
- **slint-interpreter**: Runtime `.slint` file compilation
- **streamkit-plugin-sdk-native**: StreamKit native plugin SDK

### Video Output

- **Pixel format**: RGBA8 (straight alpha)
- **Resolution**: Configurable (default 640x480)
- **Frame rate**: Configurable (default 30 fps)

### No Models Required

Unlike other StreamKit plugins, the Slint plugin has no ML models to download.
The `.slint` design files are provided as part of the pipeline configuration.

## License

- **Code**: MPL-2.0 (StreamKit Contributors)
- **Slint**: Dual-licensed under GPLv3 and Slint Commercial License
