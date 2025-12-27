<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# StreamKit Plugin SDKs

StreamKit supports two types of plugins with different trade-offs:

| Feature | Native Plugins | WASM Plugins |
|---------|---------------|--------------|
| **Performance** | Highest | Higher overhead |
| **Safety** | Trusted code only | Sandboxed execution |
| **Binary Compatibility** | C ABI (stable) | WASM (stable) |
| **Best For** | Performance-critical | Community plugins |

## Native Plugins (`native/`)

The `native/` directory contains the SDK for writing high-performance native plugins using a stable C ABI:

- `streamkit-plugin-sdk-native` crate provides an ergonomic Rust API
- Compiles to stable C ABI for binary compatibility across Rust versions
- Can link against C/C++ libraries (libopus, libx264, etc.)
- See `examples/plugins/gain-native/` for a complete example

## WASM Plugins (`wasm/`)

The `wasm/` directory packages WIT-generated bindings for WASM plugins:

- `wasm/rust/` exposes the `streamkit-plugin-sdk-wasm` crate with pre-generated component bindings
- `go/` contains generated Go bindings used by `examples/plugins/gain-wasm-go`
- `c/` provides generated C bindings used by `examples/plugins/gain-wasm-c`
- All bindings are generated from `/wit/plugin.wit`
- WASM plugins must be built as WebAssembly *components* (Component Model)
- See `examples/plugins/gain-wasm-rust/` for a complete Rust example

## Regenerating bindings

After editing the shared WIT interfaces, regenerate the language bindings and bundled
`.witpkg` via:

```bash
just gen-plugin-bindings
```

The recipe requires `wkg`, `wit-bindgen` (CLI), and the Go toolchain (with
`wit-bindgen-go` installed via `go install`) to be available in `$PATH`. The command
updates:
- `plugin-sdk/wasm/rust/src/generated/` - Rust WASM SDK bindings
- `plugin-sdk/go/bindings/` - Go bindings
- `plugin-sdk/c/include/` and `plugin-sdk/c/src/` - C bindings
- `plugin-sdk/wit/streamkit-plugin.wasm` - Binary WIT package

> **Note:** The `streamkit-plugin.wasm` file and the C bindings are pre-generated artifacts
> committed to the repository for convenience. They should not be edited manually—regenerate
> them using the command above whenever the WIT definitions in `/wit/plugin.wit` are modified.
