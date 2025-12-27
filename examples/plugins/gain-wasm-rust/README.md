<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Gain Filter Plugin Example

A simple audio gain (volume) filter plugin for StreamKit, demonstrating how to write WASM plugins.

## What it does

This plugin applies a gain adjustment to audio streams. You can specify the gain in decibels (dB), where:
- 0 dB = no change (unity gain)
- Positive values = increase volume
- Negative values = decrease volume

## Building

You need the WebAssembly Component Model toolchain:

```bash
cargo install cargo-component
rustup target add wasm32-wasip1
```

Then build the plugin:

```bash
cargo component build --release
```

The compiled plugin will be at:
```
target/wasm32-wasip1/release/gain_plugin.wasm
```

## Using with StreamKit

1. Upload the component to a running server (recommended), or copy it into your configured plugin directory (default: `.plugins/wasm/`).

2. Define a pipeline that uses the gain filter:

```yaml
steps:
  - kind: streamkit::http_input
  - kind: containers::ogg::demuxer
  - kind: audio::opus::decoder
  - kind: plugin::wasm::gain_filter_rust
    params:
      gain_db: -6.0  # Reduce volume by 6dB
  - kind: audio::opus::encoder
  - kind: containers::ogg::muxer
  - kind: streamkit::http_output
```

3. Once loaded, the node is available as `plugin::wasm::gain_filter_rust`.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `gain_db` | number | 0.0 | Gain adjustment in decibels (-60 to +20) |

## Code Structure

The plugin demonstrates:
- Implementing the `Guest` trait from `streamkit-plugin-sdk`
- Defining node metadata (inputs, outputs, parameters)
- Parsing JSON parameters
- Processing audio packets
- Logging to the host
- Sending output packets

See `src/lib.rs` for the full implementation.

## Testing with the StreamKit Server

The StreamKit server exposes REST endpoints to manage plugins at runtime. Once
the plugin is built, you can load it and exercise a stateless pipeline end-to-end:

1. **Start the server** (from the workspace root):

   ```bash
   just skit serve
   ```

2. **Upload the plugin** using the new API:

   ```bash
   curl -f -X POST \
     -F plugin=@target/wasm32-wasip1/release/gain_plugin.wasm \
     http://127.0.0.1:4545/api/v1/plugins
   ```

   The server persists the file under `.plugins/wasm/`, registers the node, and updates the available node list immediately.

3. **Run a oneshot pipeline** (this uses the freshly loaded `plugin::wasm::gain_filter_rust` node):

   ```bash
   curl -f -X POST \
     -F config=@../../samples/pipelines/oneshot/gain_filter_rust.yml \
     -F media=@../../samples/audio/system/sample.ogg \
     http://127.0.0.1:4545/api/v1/process \
     --output gain-filtered.ogg
   ```

   Note: this pipeline requires the plugin to be loaded first.

4. **Unload the plugin** when you are finished:

  ```bash
  curl -f -X DELETE http://127.0.0.1:4545/api/v1/plugins/plugin%3A%3Awasm%3A%3Again_filter_rust
  ```

   Append `?keep_file=true` to the URL if you want to keep the `.wasm` on disk while
   removing the node from the registry.

These API endpoints underpin the new UI controls as well—refreshing the browser after
uploading will surface the `plugin::gain_filter_rust` node in the palette with a plugin badge.

Once the node is running in a live pipeline you can tune its parameters in real time.
The UI sends incremental `UpdateParams` control messages (e.g. `{ "gain_db": -3.0 }`)
that the plugin receives through the new `update_params` callback. In the editor the
parameter appears as a slider, mirroring the built-in audio gain node for quick tweaks.
