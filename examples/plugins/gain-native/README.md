<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Native Gain Plugin Example

This is a simple example of a native plugin for StreamKit that applies gain (volume adjustment) to audio.

## Building

```bash
cargo build --release
```

The compiled plugin will be at `target/release/libgain_plugin_native.so` (Linux), `libgain_plugin_native.dylib` (macOS), or `gain_plugin_native.dll` (Windows).

## Loading

Upload it via the HTTP API (recommended), or copy it into your configured plugin directory (default: `.plugins/native/`):

```bash
curl -X POST -F plugin=@target/release/libgain_plugin_native.so http://127.0.0.1:4545/api/v1/plugins
```

## Features

- **Native performance**: Runs outside a sandbox (trusted code only)
- **Hot-reloadable**: Can be loaded/unloaded at runtime without restarting the server
- **Parameter updates**: Supports real-time parameter changes via `TuneNode` messages
