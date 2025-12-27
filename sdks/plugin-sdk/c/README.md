<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# StreamKit C Plugin SDK

Pre-generated C bindings for writing StreamKit plugins using the WebAssembly Component Model.

## Directory Structure

```
plugin-sdk/c/
├── include/
│   └── plugin.h       # Type definitions and function declarations
└── src/
    ├── plugin.c       # Component Model glue code
    └── plugin_component_type.o  # Pre-compiled component type object
```

## Usage

### Prerequisites

- **WASI SDK** (tested with wasi-sdk-24): https://github.com/WebAssembly/wasi-sdk/releases
- **wit-bindgen** (for regenerating bindings): `cargo install wit-bindgen-cli`

### Writing a Plugin

1. **Include the SDK header:**
   ```c
   #include "plugin.h"
   ```

2. **Implement the required exports:**

   ```c
   // Export node metadata
   void exports_streamkit_plugin_node_metadata(
       exports_streamkit_plugin_node_node_metadata_t *ret);

   // Constructor - creates a new plugin instance
   exports_streamkit_plugin_node_own_node_instance_t
   exports_streamkit_plugin_node_constructor_node_instance(
       plugin_string_t *maybe_params);

   // Process incoming packets
   bool exports_streamkit_plugin_node_method_node_instance_process(
       exports_streamkit_plugin_node_borrow_node_instance_t self,
       plugin_string_t *input_pin,
       exports_streamkit_plugin_node_packet_t *packet,
       plugin_string_t *err);

   // Update parameters dynamically
   bool exports_streamkit_plugin_node_method_node_instance_update_params(
       exports_streamkit_plugin_node_borrow_node_instance_t self,
       plugin_string_t *maybe_params,
       plugin_string_t *err);

   // Cleanup on shutdown
   void exports_streamkit_plugin_node_method_node_instance_cleanup(
       exports_streamkit_plugin_node_borrow_node_instance_t self);

   // Destructor (called when resource handle is dropped)
   void exports_streamkit_plugin_node_node_instance_destructor(
       exports_streamkit_plugin_node_node_instance_t *rep);
   ```

3. **Compile with WASI SDK:**

   Example Makefile:
   ```makefile
   WASI_SDK ?= /opt/wasi-sdk
   SDK_DIR = ../../plugin-sdk/c

   plugin.wasm: my_plugin.c
       $(WASI_SDK)/bin/clang \
           --target=wasm32-wasip2 \
           -mexec-model=reactor \
           -I$(SDK_DIR)/include \
           -o $@ \
           my_plugin.c \
           $(SDK_DIR)/src/plugin.c \
           $(SDK_DIR)/src/plugin_component_type.o
   ```

## Key Concepts

### Resource Management

The constructor must use the Component Model's resource system:

```c
// WRONG - don't manually cast pointers to handles:
handle.__handle = (int32_t)(uintptr_t)state;

// CORRECT - use the generated resource creation function:
return exports_streamkit_plugin_node_node_instance_new(
    (exports_streamkit_plugin_node_node_instance_t*)state);
```

### String Handling

- **`plugin_string_set()`**: Assigns a pointer to a const string (use for temporary/local strings)
- **`plugin_string_dup()`**: Allocates memory and copies the string (use for return values)
- **`plugin_string_free()`**: Deallocates a duplicated string

**Important:** Always use `plugin_string_dup()` for strings returned from exported functions, as the Component Model's post-return cleanup will call `free()` on them.

### Host Functions

Available host functions to call from your plugin:

```c
// Send output packets
bool streamkit_plugin_host_send_output(
    plugin_string_t *pin_name,
    streamkit_plugin_types_packet_t *packet,
    plugin_string_t *err);

// Log messages
void streamkit_plugin_host_log(
    streamkit_plugin_host_log_level_t level,
    plugin_string_t *message);
```

Log levels: `DEBUG`, `INFO`, `WARN`, `ERROR`

## Example

See `examples/plugins/gain-wasm-c/` for a complete working example.

## Regenerating Bindings

If you modify the WIT interface definitions in `/wit/plugin.wit`, regenerate the C bindings:

```bash
just gen-plugin-bindings
```

This will regenerate bindings for Rust, Go, and C.
