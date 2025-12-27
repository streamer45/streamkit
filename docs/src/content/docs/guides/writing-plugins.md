---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Writing Plugins
description: Create custom processing nodes for StreamKit
---

StreamKit supports two plugin types:

| Feature | Native Plugins | WASM Plugins |
|---------|---------------|--------------|
| Performance | Highest | Higher overhead |
| Safety | Trusted code only | Sandboxed execution |
| Best for | Private/ops plugins | Community plugins |
| Languages | Rust (FFI-friendly) | Rust, Go (TinyGo), C |
| Hot reload | Yes | Yes |

Both plugin types are uploaded via `POST /api/v1/plugins` (multipart field name `plugin`). The server detects plugin type by file extension:

- Native: `.so` / `.dylib` / `.dll` → registered as `plugin::native::<kind>`
- WASM: `.wasm` → registered as `plugin::wasm::<kind>`

## Security note

Runtime plugin upload is powerful and dangerous:

- Native plugins are arbitrary code execution in the server process.
- StreamKit does not implement authentication; use an authenticating reverse proxy and a trusted role header for access control.
- HTTP plugin upload/delete is globally disabled by default. To enable it, set `[plugins].allow_http_management = true` and ensure only trusted callers have the `load_plugins` / `delete_plugins` permissions.

See the [Security guide](/guides/security/) for recommended deployment patterns.

Uploaded plugins are stored under your configured plugin directory (default: `.plugins/`), in subfolders:

- `.plugins/native/`
- `.plugins/wasm/`

## Native Plugins

Native plugins use a stable C ABI for maximum performance.

### Project Setup

```bash
cargo new --lib my-plugin
cd my-plugin
```

```toml
# Cargo.toml
[package]
name = "my-plugin"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
streamkit-plugin-sdk-native = "0.1.0"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
```

> [!TIP]
> Pin the SDK version to match your StreamKit server/plugin ABI expectations.

### Implementation

```rust
use streamkit_plugin_sdk_native::prelude::*;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct GainParams {
    #[serde(default = "default_gain")]
    gain_db: f32,
}

fn default_gain() -> f32 { 0.0 }

struct GainPlugin {
    gain_linear: f32,
}

impl NativeProcessorNode for GainPlugin {
    fn metadata() -> NodeMetadata {
        NodeMetadata::builder("gain")
            .input("in", &[PacketType::Any])
            .output("out", PacketType::Any)
            .category("audio")
            .category("filters")
            .build()
    }

    fn new(params: Option<serde_json::Value>, logger: Logger) -> Result<Self, String> {
        let config = params
            .map(|p| serde_json::from_value::<GainParams>(p).map_err(|e| e.to_string()))
            .transpose()?
            .unwrap_or(GainParams { gain_db: 0.0 });

        let gain_linear = 10f32.powf(config.gain_db / 20.0);
        plugin_info!(logger, "gain initialized: {} dB", config.gain_db);
        Ok(Self { gain_linear })
    }

    fn process(&mut self, _pin: &str, packet: Packet, output: &OutputSender) -> Result<(), String> {
        match packet {
            Packet::Audio(mut frame) => {
                for sample in &mut frame.samples {
                    *sample *= self.gain_linear;
                }
                output.send("out", &Packet::Audio(frame))?;
                Ok(())
            }
            other => output.send("out", &other),
        }
    }
}

native_plugin_entry!(GainPlugin);
```

### Emitting Telemetry (Native)

Native plugins can emit out-of-band telemetry events to the session telemetry bus (used by the web UI timeline and streamed as WebSocket `nodetelemetry` events):

```rust
use serde_json::json;

output.emit_telemetry(
    "my_plugin.event",
    &json!({ "correlation_id": "turn-123", "detail": "something happened" }),
    None, // timestamp_us (optional)
)?;
```

Telemetry is best-effort: it should never block or stall the main audio/data path.

### Build and Load

```bash
# Build
cargo build --release

# Upload to server (multipart field name must be "plugin")
curl -X POST \
  -F plugin=@target/release/libmy_plugin.so \
  http://localhost:4545/api/v1/plugins
```

The node is now available as `plugin::native::gain` (the server applies the `plugin::native::` prefix).

## WASM Plugins

WASM plugins run in a sandboxed WebAssembly Component Model runtime.

### Project Setup

```bash
cargo new --lib my-wasm-plugin
cd my-wasm-plugin
cargo install cargo-component
```

```toml
# Cargo.toml
[package]
name = "my-wasm-plugin"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
streamkit-plugin-sdk-wasm = "0.1.0"
wit-bindgen = "0.44"
serde_json = "1"
```

> [!TIP]
> Pin the SDK version to match your StreamKit server/plugin ABI expectations.

### Build and Load

```bash
# Build
cargo component build --release

# Upload
curl -X POST \
  -F plugin=@target/wasm32-wasip1/release/my_wasm_plugin.wasm \
  http://localhost:4545/api/v1/plugins
```

## Plugin API Reference

For complete, working examples:

- `examples/plugins/gain-native`
- `examples/plugins/gain-wasm-rust`
- `examples/plugins/gain-wasm-go`
- `examples/plugins/gain-wasm-c`

## Next Steps

- [Node Reference](/reference/nodes/) - Built-in node documentation
