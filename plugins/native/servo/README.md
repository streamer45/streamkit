<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Servo Web Renderer Plugin

Renders web pages to RGBA8 video frames via the [Servo](https://servo.org/)
browser engine (v0.1.0). This is a **native plugin** — it builds as a shared
library (`.so`) loaded by StreamKit at runtime.

## Architecture

```
┌─────────────────┐       mpsc          ┌──────────────────────────────┐
│  ServoSourceNode │ ──── work items ──► │  Shared Servo Thread         │
│  (per pipeline   │ ◄─── frames ─────  │  (process-global singleton)  │
│   instance)      │                     │                              │
│                  │                     │  Servo ─┬─ WebView A         │
│  Implements      │                     │         ├─ WebView B         │
│  NativeSourceNode│                     │         └─ WebView C         │
└─────────────────┘                     └──────────────────────────────┘
```

- **Process-global singleton**: Servo's `Opts` is a global singleton — only one
  `Servo` instance exists per process. Multiple nodes share it, each with their
  own `SoftwareRenderingContext` + `WebView`.
- **Dedicated thread**: Servo types are `!Send`/`!Sync`. All Servo work runs on
  a single `std::thread`, communicated with via `std::sync::mpsc` channels.
- **Software rendering**: Uses `SoftwareRenderingContext` (llvmpipe) for fully
  headless operation — no X11, Wayland, or GPU required.

## System Dependencies

Servo embeds a full browser engine (WebRender, SpiderMonkey, etc.) and
requires several system libraries at build time.

### Ubuntu / Debian

```bash
sudo apt-get install -y \
  build-essential g++ clang cmake pkg-config libclang-dev \
  libfontconfig1-dev libharfbuzz-dev libfreetype6-dev \
  libegl1-mesa-dev libgl1-mesa-dev libgbm-dev libdrm-dev \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad \
  libvulkan1 libvulkan-dev \
  libx11-dev libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libxkbcommon-dev libwayland-dev \
  libunwind-dev llvm-dev nasm yasm \
  libssl-dev libdbus-1-dev
```

> **Important:** The `g++` package (or `libstdc++-dev`) is required — the
> `mozangle` crate compiles C++ code that includes standard library headers
> like `<array>`. If you see `fatal error: 'array' file not found` during
> build, install `g++`. The build target auto-detects GCC include paths for
> clang/bindgen.

## Build & Install

```bash
# Build the plugin
just build-plugin-native-servo

# Install to the runtime plugins directory
just install-plugin servo

# Start StreamKit
just skit
```

The plugin binary is approximately **148 MB** — this is inherent to embedding
a full browser engine with SpiderMonkey (JavaScript), WebRender (2D rendering),
and their transitive dependencies.

## Docker

Add the system dependencies to your Dockerfile before building the plugin:

```dockerfile
# Install Servo build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential g++ clang cmake pkg-config libclang-dev \
    libfontconfig1-dev libharfbuzz-dev libfreetype6-dev \
    libegl1-mesa-dev libgl1-mesa-dev libgbm-dev libdrm-dev \
    libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
    gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad \
    libvulkan1 libvulkan-dev \
    libx11-dev libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
    libxkbcommon-dev libwayland-dev \
    libunwind-dev llvm-dev nasm yasm \
    libssl-dev libdbus-1-dev \
  && rm -rf /var/lib/apt/lists/*

# Build the servo plugin
RUN just build-plugin-native-servo && just install-plugin servo
```

At runtime the `.so` dynamically links against `libfontconfig`, `libfreetype`,
and `libstdc++` — ensure these are present in the final stage of a multi-stage
build.

## Configuration

| Parameter      | Type    | Default | Description |
|----------------|---------|---------|-------------|
| `url`          | string  | —       | URL to render (required) |
| `width`        | integer | 1280    | Output frame width in pixels |
| `height`       | integer | 720     | Output frame height in pixels |
| `fps`          | integer | 30      | Output frame rate |
| `custom_css`   | string  | —       | Optional CSS injected into the page |
| `frame_count`  | integer | 0       | Total frames to generate (0 = infinite) |
| `load_timeout_secs` | integer | 30 | Maximum seconds to wait for page load |

### Runtime Updates

The `url` and `custom_css` parameters can be updated at runtime via the
WebSocket API or the controls panel. Dimensions and FPS are fixed at
creation time (the rendering context cannot be resized).

## Usage

### Oneshot — Capture a web page to video

```yaml
nodes:
  web:
    kind: plugin::native::servo
    params:
      url: "https://example.com"
      width: 1280
      height: 720
      fps: 30
      frame_count: 30  # 1 second at 30fps
```

See `samples/pipelines/oneshot/web_capture.yml` for a full pipeline.

### Dynamic — Live web overlay via MoQ

```yaml
nodes:
  web_overlay:
    kind: plugin::native::servo
    params:
      url: "https://streamkit.dev"
      width: 640
      height: 480
      fps: 30
```

See `samples/pipelines/dynamic/video_moq_servo_web_overlay.yml` for a full
pipeline compositing a web page as PiP over colorbars with MoQ streaming.

## Sample Pipelines

| Pipeline | Mode | Description |
|----------|------|-------------|
| `oneshot/web_capture.yml` | Oneshot | Render a URL to VP9/WebM video |
| `oneshot/web_pip_compositor.yml` | Oneshot | Servo WebGL PiP over colorbars → H.264/MP4 |
| `dynamic/video_moq_servo_web_overlay.yml` | Dynamic | Live web overlay composited and streamed via MoQ |

## Known Limitations

- **CPU readback only** — frames are read from the software renderer via
  `read_to_image()`. A future DMA-BUF zero-copy path will eliminate this.
- **WebGL performance** — software rendering (llvmpipe) handles WebGL but
  at reduced frame rates (~15-20 fps for complex scenes). Static HTML/CSS
  pages render at full configured FPS.
- **No input forwarding** — the rendered page is view-only. Mouse/keyboard
  interaction and a JavaScript bridge are planned for a future phase.
- **Single process** — Servo runs in-process (no multi-process sandboxing).
- **Binary size** — ~148 MB due to embedding SpiderMonkey, WebRender, and
  their transitive dependency trees.
