<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Go Gain Filter Plugin Example

This directory contains a Go implementation of the StreamKit gain (volume)
filter plugin. It mirrors the Rust example so you can compare language
ergonomics when targeting WebAssembly components.

> ℹ️ TinyGo (>= v0.34) and `wit-bindgen-go` expose first-class support for the
> WebAssembly Component Model and are now the recommended toolchain for Go
> guests. The walkthrough below follows the process documented in
> [The WebAssembly Component Model – Go](https://component-model.bytecodealliance.org/language-support/go.html).

## What it does

The node accepts raw float32 audio on its `in` pin, applies a linear gain, and
forwards the processed audio to the `out` pin. Parameters are supplied as JSON
with a single `gain_db` property identical to the Rust sample.

## Prerequisites

Make sure the following tooling is available:

1. **TinyGo** >= v0.34 with the `wasip2` target enabled.
2. **wit-bindgen-go** (ships as a Go tool) to generate bindings from the WIT definitions.
3. **wasm-tools** >= v1.0.47 (TinyGo shells out to it for componentization).
4. Optional but convenient: **wkg** (from `wasm-pkg-tools`) to bundle WIT sources.

On macOS with Homebrew that typically looks like:

```bash
brew install tinygo wasm-tools
go install go.bytecodealliance.org/cmd/wit-bindgen-go@latest
cargo install --locked wkg  # or use cargo-binstall
```

Adjust paths/platforms as needed.

> TinyGo needs an external `wasm-tools` binary even though `wit-bindgen-go`
> vendors its own copy. Keep the CLI updated to avoid mismatches.

## Bindings

The Go bindings are pre-generated and published under the shared plugin SDK at
`github.com/streamkit/streamkit-codex/plugin-sdk/go`. Add the dependency to this
module (and a local `replace` while working inside the repository) and run
`go mod tidy`. No local `wit-bindgen-go` invocation is required unless you are
refreshing the SDK itself.

Helper constructors such as `types.PacketTypeRawAudio` come directly from the
generated code.

## Build the component

Compile the guest directly to a component with TinyGo:

```bash
tinygo build \
  -target=wasip2 \
  -no-debug \
  --wit-package ../../plugin-sdk/wit/streamkit-plugin.wasm \
  --wit-world plugin \
  -o build/gain_plugin_go.wasm \
  .
```

TinyGo embeds the WIT package, resolves imports, and emits a component that
implements the `streamkit:plugin/plugin` world. The resulting
`build/gain_plugin_go.wasm` can be dropped into the `.plugins/wasm/` directory (default) or
uploaded through the StreamKit API just like the Rust variant.

## Using with StreamKit

Once the component is built:

1. Copy `build/gain_plugin_go.wasm` into your StreamKit plugin directory (default: `.plugins/wasm/`), or upload it via HTTP.
2. Start the server (`cargo run -p streamkit-server -- serve`).
3. Upload the component with the REST API if you prefer automatic registration:

   ```bash
   curl -f -X POST \
     -F plugin=@build/gain_plugin_go.wasm \
     http://127.0.0.1:4545/api/v1/plugins
   ```

4. Run the sample pipeline in `samples/pipelines/gain_filter.yml` to verify the
   output matches the Rust plugin.

## Notes

- The `//go:build tinygo.wasm` guard keeps this package out of regular host
  builds while making it obvious that TinyGo is required.
- `plugin.go` only depends on the shared SDK plus a few standard library
  packages, so `go mod tidy` is sufficient to pull everything in.
- To refresh bindings after editing the shared WIT world, run
  `just gen-plugin-bindings` from the repository root; that regenerates the Go
  and Rust SDK outputs in one shot.
