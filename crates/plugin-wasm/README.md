<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# StreamKit WASM Plugin Runtime

This crate provides WASM plugin support for StreamKit using the WebAssembly Component Model.

## How it Works

- WASM plugins define a node kind (a simple name like `gain`)
- StreamKit registers it under `plugin::wasm::<kind>` (e.g. `plugin::wasm::gain`)
- Plugins are loaded from disk at startup and can be uploaded at runtime via the HTTP API

## Uploading a Plugin

```bash
curl -X POST \
  -F plugin=@path/to/plugin.wasm \
  http://127.0.0.1:4545/api/v1/plugins
```

List loaded plugins:

```bash
curl http://127.0.0.1:4545/api/v1/plugins
```

## Examples

See:

- `examples/plugins/gain-wasm-rust`
- `examples/plugins/gain-wasm-go`
- `examples/plugins/gain-wasm-c`

## WIT Definitions

The interface definitions live in `wit/plugin.wit`.
