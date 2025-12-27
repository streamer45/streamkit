<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# C Gain Filter Plugin Example

This directory contains a pure C implementation of the StreamKit gain (volume) filter plugin. It demonstrates how to write native plugins using the stable C ABI, without any Rust code.

## What it does

The plugin applies a gain (volume adjustment) in decibels to incoming audio samples:
- Accepts raw float32 audio on its `in` pin
- Applies a configurable gain multiplier
- Outputs processed audio on its `out` pin

## Files

| File | Description |
|------|-------------|
| `streamkit_plugin.h` | C header defining the StreamKit native plugin ABI |
| `gain_plugin.c` | Plugin implementation |
| `Makefile` | Build configuration |

## Building

### Prerequisites

- A C compiler (GCC, Clang, or MSVC)
- GNU Make

### Build Commands

```bash
# Build release version
make

# Build with debug symbols
make debug

# Clean build artifacts
make clean

# Install to .plugins/native/
make install
```

The compiled plugin will be in `build/`:
- Linux: `build/libgain_plugin_c.so`
- macOS: `build/libgain_plugin_c.dylib`
- Windows: `build/gain_plugin_c.dll`

### Using just

From the repository root:

```bash
# Build
just build-plugin-native-c-gain

# Build and install
just build-plugin-native-c-gain && just install-plugins
```

## Using the Plugin

### Upload via REST API

```bash
# Upload the plugin
curl -X POST -F plugin=@build/libgain_plugin_c.so \
  http://127.0.0.1:4545/api/v1/plugins

# Verify it's loaded
curl http://127.0.0.1:4545/api/v1/plugins | jq '.[] | select(.kind | contains("gain_c"))'
```

### Use in Pipeline

```yaml
nodes:
  - id: gain
    kind: plugin::native::gain_c
    params:
      gain: 2.0  # Double the volume (+6 dB)
```

### Parameters

| Parameter | Type | Default | Range | Description |
|-----------|------|---------|-------|-------------|
| `gain` | float | 1.0 | 0.0 to 4.0 | Linear gain multiplier (tunable via UI slider) |

**Gain values:**
- `0.0` = mute (silence)
- `0.5` = -6 dB (half volume)
- `1.0` = unity (no change)
- `2.0` = +6 dB (double volume)
- `4.0` = +12 dB (maximum)

## C ABI Overview

Native plugins must export a single symbol `streamkit_native_plugin_api` that returns a pointer to a `CNativePluginAPI` struct:

```c
#include "streamkit_plugin.h"

static const CNativePluginAPI g_plugin_api = {
    .version = STREAMKIT_NATIVE_PLUGIN_API_VERSION,
    .get_metadata = my_get_metadata,
    .create_instance = my_create_instance,
    .process_packet = my_process_packet,
    .update_params = my_update_params,
    .flush = my_flush,
    .destroy_instance = my_destroy_instance
};

STREAMKIT_PLUGIN_ENTRY(&g_plugin_api)
```

### Required Functions

| Function | Description |
|----------|-------------|
| `get_metadata()` | Return static metadata about the plugin (name, pins, schema) |
| `create_instance(params, log_cb, log_data)` | Create a new instance with JSON parameters |
| `process_packet(handle, pin, packet, out_cb, out_data)` | Process an incoming packet |
| `update_params(handle, params)` | Update runtime parameters |
| `flush(handle, out_cb, out_data)` | Flush any buffered data |
| `destroy_instance(handle)` | Clean up and free resources |

### Key Types

```c
// Result type for all fallible operations
typedef struct CResult {
    bool success;
    const char* error_message;  // NULL on success
} CResult;

// Audio frame (for RawAudio packets)
typedef struct CAudioFrame {
    uint32_t sample_rate;
    uint16_t channels;
    const float* samples;  // Interleaved samples, borrowed
    size_t sample_count;   // Total samples across all channels
} CAudioFrame;

// Generic packet container
typedef struct CPacket {
    CPacketType packet_type;
    const void* data;      // Type-specific data
    size_t len;
} CPacket;
```

### Memory Management

- **Input data is borrowed**: The `samples` pointer in `CAudioFrame` is owned by the host. Do not free it.
- **Output data must be valid during callback**: When calling `output_callback`, your data must remain valid until the callback returns.
- **Plugin state**: Allocate in `create_instance`, free in `destroy_instance`.

## Comparison with Rust SDK

| Aspect | C (this example) | Rust SDK |
|--------|------------------|----------|
| Lines of code | ~300 | ~100 |
| Dependencies | None (libc only) | serde, serde_json |
| Memory safety | Manual | Automatic |
| JSON parsing | Manual (or library) | Automatic with serde |
| Macros | Simple | Full abstraction |

The C approach requires more boilerplate but gives you:
- No runtime dependencies beyond libc
- Full control over memory allocation
- Easy integration with existing C libraries
- Works with any C compiler

## See Also

- [Rust gain plugin](../gain-native/) - Same functionality in Rust
- [Go WASM plugin](../gain-wasm-go/) - WebAssembly variant in Go
- [Native Plugin SDK](../../../plugin-sdk/native/) - Rust SDK source
