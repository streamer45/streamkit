<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Servo Web Renderer Plugin

Renders web pages to RGBA8 video frames via the [Servo](https://servo.org/)
browser engine. This is a **native plugin** — it builds as a shared library
(`.so`) loaded by StreamKit at runtime.

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
> build, install `g++`.

### Build

```bash
just build-plugin-native-servo
```

### Runtime: Static TLS

Servo's dependency tree (rayon, crossbeam, tokio, parking\_lot, etc.)
uses ~4.8 KiB of `thread_local!` storage, which exceeds glibc's default
`dlopen` surplus (~512 bytes).  Without a workaround, the plugin fails
to load with `cannot allocate memory in static TLS block`.

**Recommended fix** — increase glibc's optional static TLS allocation:

```bash
GLIBC_TUNABLES=glibc.rtld.optional_static_tls=0x2000 just skit
```

**Alternative** — preload the library so it gets static TLS at startup:

```bash
LD_PRELOAD=$(pwd)/.plugins/native/servo/libservo_web.so just skit
```

> This is a known limitation of Rust's pre-compiled libstd using the
> `initial-exec` TLS model.  A future nightly-only fix would be to
> rebuild with `-Z tls-model=global-dynamic` or `-Z build-std`.

### Binary Size

The built `.so` is approximately **148 MB** — this is inherent to embedding
a full browser engine with SpiderMonkey (JavaScript), WebRender (2D
rendering), and all their transitive dependencies.

## Usage

```yaml
nodes:
  web:
    kind: plugin::native::servo
    params:
      url: "https://example.com"
      width: 1280
      height: 720
      fps: 30
      custom_css: "body { background: transparent; }"  # optional
      frame_count: 0  # 0 = infinite, >0 = stop after N frames
```

See `samples/pipelines/oneshot/web_capture.yml` for a complete example.
