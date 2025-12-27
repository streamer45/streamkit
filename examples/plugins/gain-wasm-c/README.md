<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# C Gain Filter Plugin Example

A C implementation of the StreamKit gain (volume) filter plugin, demonstrating how to write WASM plugins using `wit-bindgen` and the Component Model.

## What it does

The node accepts raw float32 audio on its `in` pin, applies a linear gain, and forwards the processed audio to the `out` pin. Functionally identical to the Rust and Go examples.

## Size Comparison

| Language   | Plugin Size | Notes |
|------------|-------------|-------|
| Rust       | 111 KB      | Recommended |
| **C**      | **~100-150 KB** | **Lowest level, manual memory management** |
| Go         | 449 KB      | Good alternative |

The C plugin is expected to be comparable to or slightly smaller than Rust, with no runtime overhead.

## Tradeoffs

### Pros
- ✅ Minimal binary size (no runtime overhead)
- ✅ Maximum performance (native code, no GC)
- ✅ Full control over memory and resources
- ✅ Easy integration of existing C/C++ DSP libraries

### Cons
- ❌ Manual memory management (risk of memory leaks/bugs)
- ❌ More verbose code (no high-level abstractions)
- ❌ Requires manual JSON parsing (no serde equivalent)
- ❌ Less integrated tooling compared to Rust's `cargo component`

## Prerequisites

### 1. Install WASI SDK (version 22+)

Download from: https://github.com/WebAssembly/wasi-sdk/releases

```bash
# Example for Linux
wget https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-24/wasi-sdk-24.0-x86_64-linux.tar.gz
tar xf wasi-sdk-24.0-x86_64-linux.tar.gz
sudo mv wasi-sdk-24.0 /opt/wasi-sdk
```

### 2. Install wasm-tools (optional, for inspection)

```bash
cargo install wasm-tools
```

## Build the component

From this directory:

```bash
make
```

This compiles `gain_plugin.c` with the pre-generated C SDK bindings from `plugin-sdk/c/`:

```bash
/opt/wasi-sdk/bin/clang \
  --target=wasm32-wasip2 \
  -mexec-model=reactor \
  -I../../plugin-sdk/c/include \
  -o build/gain_plugin_c.wasm \
  gain_plugin.c \
  ../../plugin-sdk/c/src/plugin.c \
  ../../plugin-sdk/c/src/plugin_component_type.o
```

**Custom WASI SDK path:**
```bash
make WASI_SDK=/custom/path/to/wasi-sdk
```

### Or use the justfile target from the repository root:

```bash
just build-plugin-wasm-c
```

## Using with StreamKit

1. Upload the component to a running server (recommended), or copy it into your configured plugin directory (default: `.plugins/wasm/`)

2. Define a pipeline that uses the gain filter:

```yaml
steps:
  - kind: streamkit::http_input
  - kind: containers::ogg::demuxer
  - kind: audio::opus::decoder
  - kind: plugin::wasm::gain_filter_c
    params:
      gain_db: -6.0  # Reduce volume by 6dB
  - kind: audio::opus::encoder
  - kind: containers::ogg::muxer
  - kind: streamkit::http_output
```

3. Once loaded, the node is available as `plugin::wasm::gain_filter_c`.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `gain_db` | number | 0.0 | Gain adjustment in decibels (-60 to +20) |

## Code Structure

The plugin demonstrates:
- Manual implementation of the Component Model `node` interface
- Exporting `metadata()` function with input/output pin definitions
- Resource-style `node-instance` with constructor, process, update-params, and cleanup methods
- Manual JSON parsing for parameters (simple string parsing)
- Processing audio packets with in-place sample modification
- Error handling via C return values (converted to WIT results)
- Logging via host import functions

Key implementation files:
- `gain_plugin.c` - Main implementation (you write this)
- `../../plugin-sdk/c/include/plugin.h` - Generated header (pre-generated from wit-bindgen)
- `../../plugin-sdk/c/src/plugin.c` - Generated bindings (pre-generated from wit-bindgen)
- `../../plugin-sdk/c/src/plugin_component_type.o` - Component metadata (pre-generated from wit-bindgen)

## Build Process Explanation

The C plugin SDK provides pre-generated bindings, similar to Rust and Go:

1. **SDK bindings**: The `plugin-sdk/c/` directory contains pre-generated C code from `wit-bindgen` that bridges your C functions to the Component Model ABI.

2. **Your implementation**: You write only your plugin logic in `gain_plugin.c`, including the header from the SDK.

3. **Compilation**: WASI SDK's clang compiles your C code with the SDK bindings, targeting `wasm32-wasip2` (WebAssembly + WASI Preview 2).

4. **Component creation**: The `-mexec-model=reactor` flag tells clang to create a library-style component (not a command-line application).

If the WIT interfaces change, regenerate the SDK bindings with `just gen-plugin-bindings` from the repository root.

## Inspecting the Component

After building, you can inspect the component:

```bash
# Verify it's a valid component
wasm-tools print build/gain_plugin_c.wasm | head -1
# Should output: (component

# View the WIT interface
wasm-tools component wit build/gain_plugin_c.wasm

# Get component info
wasm-tools component new --help
```

## Development Tips

### Memory Management
- Always `free()` allocated memory in the cleanup function
- Be careful with string lifetime - WIT strings are borrowed
- Use `malloc/free` for dynamic allocations

### Debugging
- Use host logging extensively: `streamkit_plugin_host_log(...)`
- Print sizes of structs during development: `sizeof(...)`
- Use `wasm-tools validate` to check component validity

### JSON Parsing
- The example uses simple `sscanf` for parsing `gain_db`
- For complex parameters, consider integrating a lightweight JSON library like `cJSON` or `parson`

## Recommendation

**When to use C:**
- Integrating existing C/C++ DSP libraries (FFT, filters, codecs)
- Maximum performance for compute-intensive audio processing
- Minimum binary size requirements
- Full control over memory layout and allocation

**When to use Rust instead:**
- General-purpose plugins (better ergonomics, safety)
- Complex state management (borrow checker prevents bugs)
- Rich JSON parsing (serde)
- Integrated tooling (`cargo component`)

**When to use Go instead:**
- Familiar to Go developers
- Built-in concurrency (goroutines) if needed
- Integrated garbage collection
- TinyGo's seamless WASM support

## Troubleshooting

### `clang: error: unable to execute command`
Make sure WASI SDK is installed:
```bash
ls /opt/wasi-sdk/bin/clang
```

Or specify custom path:
```bash
make WASI_SDK=/path/to/wasi-sdk
```

### Compilation errors about missing SDK files
The SDK bindings may not be generated yet. From the repository root:
```bash
just gen-plugin-bindings
```

### Runtime errors / crashes
Check host logs for error messages. Common issues:
- NULL pointer dereferences
- Memory leaks
- Incorrect WIT type handling
- Not using `plugin_string_dup()` for return values
- Not using `exports_streamkit_plugin_node_node_instance_new()` in the constructor
