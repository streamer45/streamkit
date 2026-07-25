<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Servo Web Renderer Plugin

Renders web pages to RGBA8 video frames via the [Servo](https://servo.org/)
browser engine (v0.4.0). This is a **native plugin** — it builds as a shared
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
| `viewport_width` | integer | 0     | Browser viewport width (0 = same as `width`). Set larger to see more of the page, scaled down. |
| `viewport_height` | integer | 0    | Browser viewport height (0 = same as `height`). Set larger to see more of the page, scaled down. |
| `viewport_resolution` | string | — | Viewport preset (`"WxH"`, e.g. `"1920x1080"`). Tunable at runtime; overrides `viewport_width`/`viewport_height`. |
| `fps`          | integer | 30      | Output frame rate |
| `custom_css`   | string  | —       | Optional CSS injected into the page |
| `frame_count`  | integer | 0       | Total frames to generate (0 = infinite) |
| `load_timeout_secs` | integer | 30 | Maximum seconds to wait for page load |
| `auth`         | object  | —       | Optional init-time authentication for private pages (see below) |

### Authentication

The optional `auth` object loads private pages non-interactively. The
credentials themselves are bound at WebView creation and are **not**
hot-swappable at runtime (changing them requires recreating the node), but the
configured request headers / bearer token are re-applied whenever the `url`
parameter is tuned at runtime, so navigating to another private page keeps
working. **Credentials are never logged**, and any `user:password@` userinfo is
stripped from URLs before they are logged.

| Field | Type | Description |
|-------|------|-------------|
| `headers` | object (string→string) | Arbitrary request headers attached to every navigation, including runtime `url` changes (e.g. `Authorization`, `Cookie`, custom `X-…`). |
| `bearer_token` | string | Convenience for `Authorization: Bearer <token>`. Conflicts with an explicit `Authorization` entry in `headers`. |
| `basic` | object | HTTP Basic/Digest credentials (`username` + `password`) answered non-interactively when the page or proxy issues an auth challenge. |
| `user_agent` | string | Custom User-Agent string. |

```yaml
nodes:
  web:
    kind: plugin::native::servo
    params:
      url: "https://private.example.com"
      auth:
        headers:
          X-Api-Key: "…"
        bearer_token: "…"            # → Authorization: Bearer …
        basic:
          username: "user"
          password: "…"
        user_agent: "StreamKit/1.0"
```

> **Global User-Agent caveat:** Servo's `Preferences` are a **process-global
> singleton**, so `auth.user_agent` is taken from the *first* registered servo
> node and applies to **all** servo nodes in the process. Per-node User-Agent
> is not currently supported. The `headers`, `bearer_token`, and `basic` fields
> are per-node.
>
> Credentials flow through pipeline YAML/params — prefer sourcing them from
> host secrets rather than committing them inline.

### Viewport Emulation

When `viewport_width`/`viewport_height` are set larger than `width`/`height`,
Servo renders the page at the viewport resolution and scales the result down
to the output frame size.  This is useful for web pages designed for wider
screens that would otherwise appear cropped in a smaller PiP window.

Example: render at 1920×1080 viewport, output as 640×360 frame:

```yaml
width: 640
height: 360
viewport_width: 1920
viewport_height: 1080
```

### Compositor Resize Hints

The plugin responds to `PreferredSize` upstream hints from the compositor.
When the compositor layer is resized, the Servo node automatically adjusts
its output dimensions to match, avoiding quality loss from compositor-level
scaling.  The viewport dimensions remain unchanged so the page layout is
not affected.

### Runtime Updates

The `url`, `custom_css`, and `viewport_resolution` parameters can be updated
at runtime via the WebSocket API or the Stream View controls panel.  Changing
`viewport_resolution` resizes the Servo rendering context, causing the page
to re-layout at the new viewport size.

The demo pipeline includes a **Viewport** dropdown with presets (480p through
1440p) so viewers can switch the viewport resolution live from Stream View.

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
      viewport_width: 1280
      viewport_height: 960
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
- **Crash recovery is partial** — Rust panics are caught via `catch_unwind`
  and the affected node falls back to its last good frame. However, native
  crashes in SpiderMonkey or Mesa/llvmpipe (SIGSEGV, SIGBUS, abort) still
  terminate the shared Servo thread and all web renderer nodes.
- **Single process** — Servo runs in-process (no multi-process sandboxing).
- **Binary size** — ~148 MB due to embedding SpiderMonkey, WebRender, and
  their transitive dependency trees.
