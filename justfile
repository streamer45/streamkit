# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
#
# SPDX-License-Identifier: MPL-2.0

# --- Generic ---
# Feature flags for builds (modify as needed)
moq_features := "--features moq"
profiling_features := "--features profiling"
tokio_console_features := "--features tokio-console"

# Optional extra features to enable in skit builds (e.g. "svt_av1").
# Usage: just extra_features="--features svt_av1" skit
#    or: just extra_features="--features svt_av1" build-skit
extra_features := ""

# sherpa-onnx version for Kokoro TTS plugin (must match sherpa-rs version)
# sherpa-rs v0.6.8 uses sherpa-onnx v1.12.17
sherpa_onnx_version := "1.12.17"

# Shared target directory for native plugin builds (deduplicates common deps)
plugins_target_dir := justfile_directory() / "target" / "plugins"

# List all available commands
default:
    @just --list

# Remove build artifacts not used in the last 7 days (use instead of cargo clean)
sweep days='7':
    @command -v cargo-sweep >/dev/null 2>&1 || { echo "Install cargo-sweep: cargo install cargo-sweep --locked"; exit 1; }
    @echo "Sweeping stale build artifacts (older than {{days}} days)..."
    @cargo sweep --time {{days}}
    @echo "Sweeping shared plugin target dir..."
    @CARGO_TARGET_DIR={{plugins_target_dir}} cargo sweep --time {{days}}

# --- Codegen ---
# Generate TypeScript types from Rust code
gen-types:
    @echo "Generating TypeScript types..."
    @cargo run -p streamkit-api --bin generate-ts-types
    @cargo run -p streamkit-nodes --bin generate-compositor-types --features codegen

# Fetch WIT dependencies (WASI interfaces)
fetch-wit-deps:
    @echo "Fetching WIT dependencies..."
    @wkg wit fetch -d wit

# Generate pre-baked bindings for WASM plugin SDKs (Rust, Go, and C)
gen-plugin-bindings: fetch-wit-deps
    @echo "Regenerating StreamKit WASM plugin bindings..."
    @wkg wit build --wit-dir wit --output sdks/plugin-sdk/wit/streamkit-plugin.wasm
    @rm -rf sdks/plugin-sdk/wasm/rust/src/generated
    @mkdir -p sdks/plugin-sdk/wasm/rust/src/generated
    @wit-bindgen rust \
        --world plugin \
        --generate-all \
        --pub-export-macro \
        --runtime-path wit_bindgen_rt \
        --bitflags-path wit_bindgen_rt::bitflags \
        --out-dir sdks/plugin-sdk/wasm/rust/src/generated \
        wit
    @printf '%s\n' \
        '#![allow(dead_code)]' \
        '#![allow(clippy::all)]' \
        '#![allow(missing_docs)]' \
        'pub mod plugin;' \
        'pub use plugin::*;' \
        > sdks/plugin-sdk/wasm/rust/src/generated/mod.rs
    @rm -rf sdks/plugin-sdk/go/bindings
    @(cd sdks/plugin-sdk/go && go tool wit-bindgen-go generate \
        --world plugin \
        --out bindings \
        ../wit/streamkit-plugin.wasm)
    @rm -rf sdks/plugin-sdk/c/include sdks/plugin-sdk/c/src
    @mkdir -p sdks/plugin-sdk/c/include sdks/plugin-sdk/c/src
    @(cd sdks/plugin-sdk/c && wit-bindgen c ../../wit --world plugin)
    @mv sdks/plugin-sdk/c/plugin.h sdks/plugin-sdk/c/include/
    @mv sdks/plugin-sdk/c/plugin.c sdks/plugin-sdk/c/plugin_component_type.o sdks/plugin-sdk/c/src/
    @cargo fmt -p streamkit-plugin-sdk-wasm
    @gofmt -w sdks/plugin-sdk/go || true

# --- skit ---
# Pre-flight: ensure the UI has been built (required by RustEmbed)
check-ui-dist:
    @if [ ! -d ui/dist ]; then \
        echo ""; \
        echo "Error: ui/dist/ does not exist."; \
        echo ""; \
        echo "The skit server embeds the web UI at compile time (RustEmbed)."; \
        echo "Build it first with:"; \
        echo ""; \
        echo "  just build-ui"; \
        echo ""; \
        exit 1; \
    fi

# Build the skit in release mode
build-skit:
    @echo "Building skit..."
    @cargo build --release {{moq_features}} {{extra_features}} -p streamkit-server --bin skit

# Build skit in release mode with native CPU optimisations
# Produces a binary tuned for the build host's microarchitecture (not portable).
build-skit-native:
    @echo "Building skit (target-cpu=native)..."
    @RUSTFLAGS="-C target-cpu=native" cargo build --release {{moq_features}} {{extra_features}} -p streamkit-server --bin skit

# Build the skit with profiling support
# Uses release-lto profile for thin LTO (eliminates UB-check overhead and enables
# cross-crate SIMD inlining), frame pointers for fast stack unwinding (required by
# pprof frame-pointer feature), and target-cpu=native so profiles reflect host-tuned codegen.
build-skit-profiling:
    @echo "Building skit with profiling support (release-lto + frame pointers + native CPU)..."
    @RUSTFLAGS="-C force-frame-pointers=yes -C target-cpu=native" cargo build --profile release-lto {{moq_features}} {{profiling_features}} -p streamkit-server --bin skit

# Start the skit server
skit *args='': check-ui-dist
    @echo "Starting skit..."
    @cargo run {{moq_features}} {{extra_features}} -p streamkit-server --bin skit -- {{args}}

# Start the skit server with profiling support (CPU + heap)
# Uses release-lto profile for thin LTO (eliminates UB-check overhead and enables
# cross-crate SIMD inlining), frame pointers for fast stack unwinding (required by
# pprof frame-pointer feature), and target-cpu=native so profiles reflect host-tuned codegen.
skit-profiling *args='':
    @echo "Starting skit with profiling support (release-lto + CPU + heap, frame pointers + native CPU)..."
    @echo "Note: Heap profiling configuration is embedded in the binary"
    @RUSTFLAGS="-C force-frame-pointers=yes -C target-cpu=native" cargo run --profile release-lto {{moq_features}} {{profiling_features}} -p streamkit-server --bin skit -- {{args}}

# Start the skit server with tokio-console support
skit-console *args='':
    @echo "Starting skit with tokio-console support..."
    @echo "Connect with: tokio-console http://localhost:6669"
    RUSTFLAGS="--cfg tokio_unstable" SK_TELEMETRY__TOKIO_CONSOLE=true cargo run {{moq_features}} {{tokio_console_features}} -p streamkit-server --bin skit -- {{args}}

# Run the skit client
skit-cli *args='':
    @cargo run -p streamkit-client --bin skit-cli -- {{args}}

# Run the load test tool (alias: lt)
skit-lt config='loadtest.toml' *args='':
    @cargo run -p streamkit-client --bin skit-cli -- loadtest {{config}} {{args}}

# Run a load test by preset id (maps to `samples/loadtest/<id>.toml`) or by explicit path.
#
# Examples:
# - `just lt`                           # runs `samples/loadtest/stress-oneshot.toml` by default
# - `just lt stress-dynamic`            # runs `samples/loadtest/stress-dynamic.toml`
# - `just lt stress-dynamic sessions=10` # shorthand for `--sessions 10`
# - `just lt stress-dynamic --sessions 10`
# - `just lt dynamic-tune-heavy --cleanup`
# - `just lt samples/loadtest/ui-demo.toml`
lt preset_or_path='stress-oneshot' *args='':
    @cfg=""
    @if [ -f "{{preset_or_path}}" ]; then \
      cfg="{{preset_or_path}}"; \
    elif [ -f "samples/loadtest/{{preset_or_path}}.toml" ]; then \
      cfg="samples/loadtest/{{preset_or_path}}.toml"; \
    else \
      echo "❌ Loadtest config not found: '{{preset_or_path}}'"; \
      echo "   - If passing a preset, expected: samples/loadtest/{{preset_or_path}}.toml"; \
      echo "   - If passing a path, ensure the file exists"; \
      exit 1; \
    fi; \
    sessions=""; \
    set -- {{args}}; \
    if [ $# -ge 1 ]; then \
      case "$1" in \
        sessions=*) sessions="${1#sessions=}"; shift;; \
        [0-9]*) sessions="$1"; shift;; \
      esac; \
    fi; \
    if [ -n "$sessions" ]; then \
      case "$sessions" in \
        ''|*[!0-9]*) echo "❌ sessions must be an integer (got: '$sessions')"; exit 1;; \
      esac; \
      set -- --sessions "$sessions" "$@"; \
    fi; \
    just skit-lt "$cfg" "$@"

# --- Load test presets ---
# Run the standard oneshot stress test config
lt-oneshot *args='':
    @just lt stress-oneshot {{args}}

# Run the standard dynamic session stress test config
lt-dynamic *args='':
    @just lt stress-dynamic {{args}}

# Run the standard dynamic session stress test config with cleanup enabled
lt-dynamic-cleanup *args='':
    @just lt stress-dynamic --cleanup {{args}}

# Run the long-running UI demo config
lt-ui-demo *args='':
    @just lt ui-demo {{args}}

# Run skit tests (skips GPU-only tests; use test-skit-gpu for those)
# Note: We exclude dhat-heap since it's mutually exclusive with profiling (both define global allocators)
test-skit:
    @echo "Testing skit..."
    @cargo test --workspace -- --skip gpu_tests::
    @cargo test -p streamkit-server --features "moq"

# Run GPU compositor tests (requires a machine with a GPU)
test-skit-gpu:
    @echo "Testing skit (GPU)..."
    @cargo test -p streamkit-nodes --features gpu
    @cargo test -p streamkit-engine --features gpu

# Lint and format check the skit code
# Note: We exclude dhat-heap since it's mutually exclusive with profiling (both define global allocators)
lint-skit:
    @echo "Linting skit..."
    @cargo fmt --all -- --check
    @cargo clippy -p streamkit-server --all-targets --features "moq" -- -D warnings
    @cargo clippy --workspace --exclude streamkit-server --all-targets -- -D warnings
    @mkdir -p target
    @HOST=$(rustc -vV | sed -n 's/^host: //p'); \
      cargo metadata --locked --format-version 1 --filter-platform "$HOST" > target/cargo-metadata.json
    @cargo deny check licenses --metadata-path target/cargo-metadata.json

# Auto-fix formatting and linting issues in skit code
# Note: We exclude dhat-heap since it's mutually exclusive with profiling (both define global allocators)
fix-skit:
    @echo "Auto-fixing skit code..."
    @cargo fmt --all
    @cargo clippy --fix --allow-dirty --allow-staged -p streamkit-server --all-targets --features "moq" -- -D warnings
    @cargo clippy --fix --allow-dirty --allow-staged --workspace --exclude streamkit-server --all-targets -- -D warnings

# --- Frontend ---
# Install UI dependencies using Bun
[working-directory: 'ui']
install-ui:
    @echo "Installing UI dependencies..."
    @mkdir -p .bun_tmp
    @mkdir -p .bun_install
    @BUN_TMPDIR=.bun_tmp BUN_INSTALL=.bun_install bun install

# Build the UI for production
[working-directory: 'ui']
build-ui: install-ui
    @echo "Building UI..."
    @bun run build

# Start the UI development server with hot reload
[working-directory: 'ui']
ui: install-ui
    @echo "Starting UI..."
    @bun run dev

# Run UI tests
[working-directory: 'ui']
test-ui: install-ui
    @echo "Testing UI..."
    @bun run test:run

# Run UI render-performance tests (Layer 1)
[working-directory: 'ui']
perf-ui: install-ui
    @echo "Running UI render-performance tests..."
    @bun run test:perf

# Run e2e compositor perf test (Layer 2)
# Requires: just skit (backend) + just ui (Vite dev server at :3045)
[working-directory: 'e2e']
perf-e2e: install-e2e
    @echo "Running e2e compositor perf test against dev server..."
    @E2E_BASE_URL=${E2E_BASE_URL:-http://localhost:3045} bunx playwright test tests/compositor-perf.spec.ts

# Analyze a React DevTools profiling export to find unnecessary re-renders
# Usage: just analyze-profile <profile.json> [options]
# Options: --top N, --commit N, --threshold MS, --filter PATTERN, --cascade, --why, --summary
analyze-profile *ARGS:
    @node scripts/analyze-react-profile.mjs {{ARGS}}

# Lint and type-check the UI code
[working-directory: 'ui']
lint-ui: install-ui
    @echo "Linting UI..."
    @bun run lint

# Auto-fix UI code formatting and linting issues
[working-directory: 'ui']
fix-ui: install-ui
    @echo "Auto-fixing UI code..."
    @bun run format
    @bun run lint:fix

# Check for unused files, exports, and dependencies in the UI code
[working-directory: 'ui']
knip-ui: install-ui
    @echo "Checking for unused code in UI..."
    @bun run knip

# --- Documentation ---
# Install documentation site dependencies
[working-directory: 'docs']
install-docs:
    @echo "Installing documentation dependencies..."
    @bun install

# Start documentation development server
[working-directory: 'docs']
docs: install-docs
    @echo "Starting documentation server at http://localhost:4321"
    @bun run dev

# Build documentation for production
[working-directory: 'docs']
build-docs: install-docs
    @echo "Building documentation..."
    @bun run build

# Preview production documentation build
[working-directory: 'docs']
preview-docs: build-docs
    @echo "Previewing documentation at http://localhost:4321"
    @bun run preview

# Generate reference docs (built-in nodes + official plugins)
gen-docs-reference:
    @echo "Generating reference documentation (nodes + plugins + packets)..."
    @cargo run -p streamkit-server --bin gen-docs-reference

# Lint native plugins
lint-plugins:
    @echo "Linting native plugins..."
    @cd plugins/native/whisper && cargo fmt -- --check && CARGO_TARGET_DIR={{plugins_target_dir}} cargo clippy -- -D warnings
    @cd plugins/native/kokoro && cargo fmt -- --check && CARGO_TARGET_DIR={{plugins_target_dir}} cargo clippy -- -D warnings
    @cd plugins/native/piper && cargo fmt -- --check && CARGO_TARGET_DIR={{plugins_target_dir}} cargo clippy -- -D warnings
    @cd plugins/native/sensevoice && cargo fmt -- --check && CARGO_TARGET_DIR={{plugins_target_dir}} cargo clippy -- -D warnings
    @cd plugins/native/vad && cargo fmt -- --check && CARGO_TARGET_DIR={{plugins_target_dir}} cargo clippy -- -D warnings
    @cd plugins/native/matcha && cargo fmt -- --check && CARGO_TARGET_DIR={{plugins_target_dir}} cargo clippy -- -D warnings
    @cd plugins/native/pocket-tts && cargo fmt -- --check && CARGO_TARGET_DIR={{plugins_target_dir}} cargo clippy -- -D warnings
    @cd plugins/native/nllb && cargo fmt -- --check && CMAKE_ARGS="-DCMAKE_INSTALL_PREFIX=$$(pwd)/target/cmake-install" CARGO_TARGET_DIR={{plugins_target_dir}} cargo clippy -- -D warnings
    @cd plugins/native/supertonic && cargo fmt -- --check && CARGO_TARGET_DIR={{plugins_target_dir}} cargo clippy -- -D warnings
    @cd plugins/native/slint && cargo fmt -- --check && CARGO_TARGET_DIR={{plugins_target_dir}} cargo clippy -- -D warnings
    @cd plugins/native/aac-encoder && cargo fmt -- --check && CARGO_TARGET_DIR={{plugins_target_dir}} cargo clippy -- -D warnings
    @echo "✓ All native plugins passed linting"

# Auto-fix formatting and linting issues in native plugins
fix-plugins:
    @echo "Auto-fixing native plugins..."
    @cd plugins/native/whisper && cargo fmt && CARGO_TARGET_DIR={{plugins_target_dir}} cargo clippy --fix --allow-dirty --allow-staged -- -D warnings
    @cd plugins/native/kokoro && cargo fmt && CARGO_TARGET_DIR={{plugins_target_dir}} cargo clippy --fix --allow-dirty --allow-staged -- -D warnings
    @cd plugins/native/piper && cargo fmt && CARGO_TARGET_DIR={{plugins_target_dir}} cargo clippy --fix --allow-dirty --allow-staged -- -D warnings
    @cd plugins/native/sensevoice && cargo fmt && CARGO_TARGET_DIR={{plugins_target_dir}} cargo clippy --fix --allow-dirty --allow-staged -- -D warnings
    @cd plugins/native/vad && cargo fmt && CARGO_TARGET_DIR={{plugins_target_dir}} cargo clippy --fix --allow-dirty --allow-staged -- -D warnings
    @cd plugins/native/matcha && cargo fmt && CARGO_TARGET_DIR={{plugins_target_dir}} cargo clippy --fix --allow-dirty --allow-staged -- -D warnings
    @cd plugins/native/pocket-tts && cargo fmt && CARGO_TARGET_DIR={{plugins_target_dir}} cargo clippy --fix --allow-dirty --allow-staged -- -D warnings
    @cd plugins/native/nllb && cargo fmt && CMAKE_ARGS="-DCMAKE_INSTALL_PREFIX=$$(pwd)/target/cmake-install" CARGO_TARGET_DIR={{plugins_target_dir}} cargo clippy --fix --allow-dirty --allow-staged -- -D warnings
    @cd plugins/native/supertonic && cargo fmt && CARGO_TARGET_DIR={{plugins_target_dir}} cargo clippy --fix --allow-dirty --allow-staged -- -D warnings
    @cd plugins/native/slint && cargo fmt && CARGO_TARGET_DIR={{plugins_target_dir}} cargo clippy --fix --allow-dirty --allow-staged -- -D warnings
    @cd plugins/native/aac-encoder && cargo fmt && CARGO_TARGET_DIR={{plugins_target_dir}} cargo clippy --fix --allow-dirty --allow-staged -- -D warnings
    @echo "✓ All native plugins fixed"

# --- Profiling ---
# Note: Profiling requires server to be running with --features profiling
# Start server with: just skit-profiling serve

# Fetch a CPU profile from running skit server (requires Go with pprof installed)
# Duration in seconds (default: 30), format: flamegraph or protobuf (default: protobuf)
profile-fetch duration='30' format='protobuf' output='profile.pb':
    @echo "Fetching {{duration}}s CPU profile in {{format}} format..."
    @echo "Note: Server must be running with profiling enabled (just skit-profiling serve)"
    @curl -s "http://127.0.0.1:4545/api/v1/profile/cpu?duration_secs={{duration}}&format={{format}}" -o {{output}} || (echo "❌ Failed to fetch profile. Is the server running with profiling enabled?" && exit 1)
    @if [ ! -s {{output}} ] || grep -q "501 Not Implemented" {{output}} 2>/dev/null; then \
        echo "❌ Profiling not enabled. Start server with: just skit-profiling serve"; \
        rm -f {{output}}; \
        exit 1; \
    fi
    @echo "✓ Profile saved to {{output}}"

# Fetch and analyze CPU profile with pprof interactive web UI (requires Go)
profile-web duration='30':
    @echo "Fetching {{duration}}s CPU profile and opening in browser..."
    @just profile-fetch {{duration}} protobuf /tmp/skit-profile.pb
    @echo "Starting pprof web UI at http://localhost:8080"
    @go tool pprof -http=:8080 /tmp/skit-profile.pb

# Fetch and generate flamegraph SVG
profile-flame duration='30' output='flamegraph.svg':
    @echo "Fetching {{duration}}s CPU profile as flamegraph..."
    @just profile-fetch {{duration}} flamegraph {{output}}
    @echo "✓ Flamegraph saved to {{output}}"
    @echo "  Open with: open {{output}} (macOS) or xdg-open {{output}} (Linux)"

# Fetch profile and show top functions (requires Go)
profile-top duration='30':
    @echo "Fetching {{duration}}s CPU profile..."
    @just profile-fetch {{duration}} protobuf /tmp/skit-profile.pb
    @echo "Top functions by CPU usage:"
    @go tool pprof -top /tmp/skit-profile.pb

# Fetch a heap profile from running skit server (requires Go with pprof installed)
heap-profile-fetch output='heap.pb.gz':
    @echo "Fetching heap profile..."
    @echo "Note: Server must be running with profiling enabled (just skit-profiling serve)"
    @curl -s "http://127.0.0.1:4545/api/v1/profile/heap" -o {{output}} || (echo "❌ Failed to fetch heap profile. Is the server running with profiling enabled?" && exit 1)
    @if [ ! -s {{output}} ] || grep -q "501 Not Implemented" {{output}} 2>/dev/null; then \
        echo "❌ Heap profiling not enabled. Start server with: just skit-profiling serve"; \
        rm -f {{output}}; \
        exit 1; \
    fi
    @echo "✓ Heap profile saved to {{output}}"

# Fetch and analyze heap profile with pprof interactive web UI (requires Go)
heap-profile-web:
    @echo "Fetching heap profile and opening in browser..."
    @just heap-profile-fetch /tmp/skit-heap.pb.gz
    @echo "Starting pprof web UI at http://localhost:8080"
    @echo "Note: Symbolization may be slow. Use Ctrl+C if it hangs."
    @go tool pprof -http=:8080 /tmp/skit-heap.pb.gz

# Fetch heap profile and show top allocations (requires Go)
heap-profile-top:
    @echo "Fetching heap profile..."
    @just heap-profile-fetch /tmp/skit-heap.pb.gz
    @echo "Top allocations by memory usage:"
    @go tool pprof -top /tmp/skit-heap.pb.gz

# --- DHAT Allocation Profiling ---
# DHAT tracks allocation counts/rates (not just live memory like jemalloc)
# Use this to find hot allocation sites that cause heap churn

# Build skit with DHAT allocation profiling enabled
build-skit-dhat:
    @echo "Building skit with DHAT allocation profiling..."
    @echo "Note: DHAT and jemalloc profiling are mutually exclusive"
    cargo build -p streamkit-server --features dhat-heap --no-default-features --features script --features moq
    @echo "✓ Built with DHAT. Run with: just skit-dhat serve"

# Run skit with DHAT profiling (writes dhat-heap.json on graceful shutdown)
skit-dhat *args:
    @echo "Running skit with DHAT allocation profiling..."
    @echo "Press Ctrl+C to stop and generate dhat-heap.json"
    cargo run -p streamkit-server --bin skit --features dhat-heap --no-default-features --features script --features moq -- {{args}}

# View DHAT output in browser (after running skit-dhat and stopping gracefully)
dhat-view:
    #!/usr/bin/env bash
    if [ ! -f dhat-heap.json ]; then
        echo "❌ dhat-heap.json not found. Run 'just skit-dhat serve' first, then stop with Ctrl+C"
        exit 1
    fi
    echo "Opening DHAT viewer in browser..."
    echo "Upload dhat-heap.json to the viewer"
    if command -v xdg-open &> /dev/null; then
        xdg-open "https://nnethercote.github.io/dh_view/dh_view.html"
    elif command -v open &> /dev/null; then
        open "https://nnethercote.github.io/dh_view/dh_view.html"
    else
        echo "Open https://nnethercote.github.io/dh_view/dh_view.html in your browser"
    fi
    echo "Then upload: $(pwd)/dhat-heap.json"

# --- Combined ---
# Build both skit and frontend
build: build-skit build-ui build-plugins

# Run all tests (skit and frontend)
test: test-skit test-ui

# Lint all code
lint: lint-skit lint-ui lint-plugins check-license-headers

# Start full development environment (skit + frontend with hot reload)
dev: install-ui
    @echo "Starting development environment..."
    @echo "Press Ctrl+C to exit."
    @trap 'kill 0' EXIT; \
    (cargo watch -x "run {{moq_features}} -p streamkit-server --bin skit -- serve") & \
    (cd ui && bun run dev)

# --- Plugins ---

## WASM Plugins

# Build Rust WASM gain plugin example
[working-directory: 'examples/plugins/gain-wasm-rust']
build-plugin-wasm-rust:
    @echo "Building Rust WASM gain plugin..."
    @cargo component build --release
    @echo "✓ Plugin built: examples/plugins/gain-wasm-rust/target/wasm32-wasip1/release/gain_plugin.wasm"

# Build Go WASM gain plugin example
[working-directory: 'examples/plugins/gain-wasm-go']
build-plugin-wasm-go:
    @echo "Building Go WASM gain plugin..."
    @mkdir -p build
    @tinygo build \
        -target=wasip2 \
        -no-debug \
        --wit-package ../../../sdks/plugin-sdk/wit/streamkit-plugin.wasm \
        --wit-world plugin \
        -o build/gain_plugin_go.wasm \
        .
    @echo "✓ Plugin built: examples/plugins/gain-wasm-go/build/gain_plugin_go.wasm"

# Build C WASM gain plugin example (requires wit-bindgen and WASI SDK)
[working-directory: 'examples/plugins/gain-wasm-c']
build-plugin-wasm-c:
    @echo "Building C WASM gain plugin..."
    @make

# Build all WASM plugin examples
build-plugins-wasm: build-plugin-wasm-rust build-plugin-wasm-go build-plugin-wasm-c

## Native Plugins

# Build native gain plugin example
[working-directory: 'examples/plugins/gain-native']
build-plugin-native-gain:
    @echo "Building native gain plugin..."
    @cargo build --release

# Download Silero VAD model for Whisper plugin
download-silero-vad:
    @echo "Downloading Silero VAD model..."
    @mkdir -p models
    @if [ -f models/silero_vad.onnx ]; then \
        echo "✓ Silero VAD model already exists at models/silero_vad.onnx"; \
    else \
        curl -L -o models/silero_vad.onnx \
            https://huggingface.co/streamkit/whisper-models/resolve/main/silero_vad.onnx && \
        echo "✓ Silero VAD model downloaded to models/silero_vad.onnx ($(du -h models/silero_vad.onnx | cut -f1))"; \
    fi

# Download Whisper models (tiny.en, base.en, base multilingual)
download-whisper-models:
    @echo "Downloading Whisper models..."
    @mkdir -p models
    @if [ -f models/ggml-tiny.en-q5_1.bin ]; then \
        echo "✓ Whisper tiny.en model already exists at models/ggml-tiny.en-q5_1.bin"; \
    else \
        echo "Downloading ggml-tiny.en-q5_1.bin (~31MB)..." && \
        curl -L -o models/ggml-tiny.en-q5_1.bin \
            https://huggingface.co/streamkit/whisper-models/resolve/main/ggml-tiny.en-q5_1.bin && \
        echo "✓ Whisper tiny.en model downloaded to models/ggml-tiny.en-q5_1.bin ($(du -h models/ggml-tiny.en-q5_1.bin | cut -f1))"; \
    fi
    @if [ -f models/ggml-base.en-q5_1.bin ]; then \
        echo "✓ Whisper base.en model already exists at models/ggml-base.en-q5_1.bin"; \
    else \
        echo "Downloading ggml-base.en-q5_1.bin (~58MB)..." && \
        curl -L -o models/ggml-base.en-q5_1.bin \
            https://huggingface.co/streamkit/whisper-models/resolve/main/ggml-base.en-q5_1.bin && \
        echo "✓ Whisper base.en model downloaded to models/ggml-base.en-q5_1.bin ($(du -h models/ggml-base.en-q5_1.bin | cut -f1))"; \
    fi
    @if [ -f models/ggml-base-q5_1.bin ]; then \
        echo "✓ Whisper base (multilingual) model already exists at models/ggml-base-q5_1.bin"; \
    else \
        echo "Downloading ggml-base-q5_1.bin (~57MB)..." && \
        curl -L -o models/ggml-base-q5_1.bin \
            https://huggingface.co/streamkit/whisper-models/resolve/main/ggml-base-q5_1.bin && \
        echo "✓ Whisper base (multilingual) model downloaded to models/ggml-base-q5_1.bin ($(du -h models/ggml-base-q5_1.bin | cut -f1))"; \
    fi

# Setup Whisper (download models + VAD)
setup-whisper: download-whisper-models download-silero-vad
    @echo "✓ Whisper STT setup complete!"

# Build native whisper STT plugin
[working-directory: 'plugins/native/whisper']
build-plugin-native-whisper:
    @echo "Building native Whisper STT plugin..."
    @CARGO_TARGET_DIR={{plugins_target_dir}} cargo build --release

# Download and install sherpa-onnx shared library (required for Kokoro plugin)
install-sherpa-onnx:
    #!/usr/bin/env bash
    set -e
    echo "Installing sherpa-onnx v{{ sherpa_onnx_version }} shared library..."
    cd /tmp
    # Download pre-built sherpa-onnx for Linux x64
    ARCHIVE="sherpa-onnx-v{{ sherpa_onnx_version }}-linux-x64-shared.tar.bz2"
    if [ ! -f "$ARCHIVE" ]; then
        wget "https://github.com/k2-fsa/sherpa-onnx/releases/download/v{{ sherpa_onnx_version }}/$ARCHIVE"
    fi
    tar xf "$ARCHIVE"
    # Install to /usr/local
    sudo cp "sherpa-onnx-v{{ sherpa_onnx_version }}-linux-x64-shared/lib/"*.so* /usr/local/lib/
    sudo ldconfig
    echo "✓ sherpa-onnx v{{ sherpa_onnx_version }} installed to /usr/local/lib"

# Download Kokoro TTS models
download-kokoro-models:
    @echo "Downloading Kokoro TTS models..."
    @mkdir -p models
    @if [ -f models/kokoro-multi-lang-v1_1.tar.bz2 ]; then \
        echo "✓ Kokoro archive already exists at models/kokoro-multi-lang-v1_1.tar.bz2"; \
    else \
        echo "Downloading kokoro-multi-lang-v1_1.tar.bz2 (~348MB)..." && \
        curl -L -o models/kokoro-multi-lang-v1_1.tar.bz2 \
            https://huggingface.co/streamkit/kokoro-models/resolve/main/kokoro-multi-lang-v1_1.tar.bz2 && \
        echo "✓ Kokoro archive downloaded"; \
    fi
    @if [ -d models/kokoro-multi-lang-v1_1 ]; then \
        echo "✓ Kokoro models already extracted at models/kokoro-multi-lang-v1_1"; \
    else \
        echo "Extracting models..." && \
        cd models && tar xf kokoro-multi-lang-v1_1.tar.bz2 && \
        echo "✓ Kokoro v1.1 models ready at models/kokoro-multi-lang-v1_1 (103 speakers, 24kHz)"; \
    fi

# Setup Kokoro TTS (install dependencies + download models)
setup-kokoro: install-sherpa-onnx download-kokoro-models
    @echo "✓ Kokoro TTS setup complete!"

# Download Piper TTS models
download-piper-models:
    @echo "Downloading Piper TTS models..."
    @cd plugins/native/piper && ./download-models.sh

# Download Pocket TTS models and voices
download-pocket-tts-models:
    @echo "Downloading Pocket TTS models and voices..."
    @echo "⚠️  This requires Python with huggingface-hub installed."
    @echo "⚠️  Install with: pip3 install --user huggingface-hub"
    @echo "⚠️  Model weights are gated; set HF_TOKEN to authenticate."
    @echo ""
    @HF_HOME=models/hf python3 plugins/native/pocket-tts/download-models.py
    @echo "✓ Pocket TTS models copied to models/pocket-tts (HF cache in models/hf)"

# Setup Piper TTS (install dependencies + download models)
setup-piper: install-sherpa-onnx download-piper-models
    @echo "✓ Piper TTS setup complete!"

# Build native Kokoro TTS plugin
[working-directory: 'plugins/native/kokoro']
build-plugin-native-kokoro:
    @echo "Building native Kokoro TTS plugin..."
    @CARGO_TARGET_DIR={{plugins_target_dir}} cargo build --release

# Upload Kokoro plugin to running server
[working-directory: 'plugins/native/kokoro']
upload-kokoro-plugin: build-plugin-native-kokoro
    @echo "Uploading Kokoro plugin to server..."
    @curl -X POST -F "plugin=@{{plugins_target_dir}}/release/libkokoro.so" \
        http://127.0.0.1:4545/api/v1/plugins

# Build native Piper TTS plugin
[working-directory: 'plugins/native/piper']
build-plugin-native-piper:
    @echo "Building native Piper TTS plugin..."
    @CARGO_TARGET_DIR={{plugins_target_dir}} cargo build --release

# Upload Piper plugin to running server
[working-directory: 'plugins/native/piper']
upload-piper-plugin: build-plugin-native-piper
    @echo "Uploading Piper plugin to server..."
    @curl -X POST -F "plugin=@{{plugins_target_dir}}/release/libpiper.so" \
        http://127.0.0.1:4545/api/v1/plugins

# Download Matcha TTS models
download-matcha-models:
    @echo "Downloading Matcha TTS models..."
    @cd plugins/native/matcha && ./download-models.sh

# Setup Matcha TTS (install dependencies + download models)
setup-matcha: install-sherpa-onnx download-matcha-models
    @echo "✓ Matcha TTS setup complete!"

# Build native Matcha TTS plugin
[working-directory: 'plugins/native/matcha']
build-plugin-native-matcha:
    @echo "Building native Matcha TTS plugin..."
    @CARGO_TARGET_DIR={{plugins_target_dir}} cargo build --release

# Build native Pocket TTS plugin
[working-directory: 'plugins/native/pocket-tts']
build-plugin-native-pocket-tts:
    @echo "Building native Pocket TTS plugin..."
    @CARGO_TARGET_DIR={{plugins_target_dir}} cargo build --release

# Upload Pocket TTS plugin to running server
[working-directory: 'plugins/native/pocket-tts']
upload-pocket-tts-plugin: build-plugin-native-pocket-tts
    @echo "Uploading Pocket TTS plugin to server..."
    @curl -X POST -F "plugin=@{{plugins_target_dir}}/release/libpocket_tts.so" \
        http://127.0.0.1:4545/api/v1/plugins

# Upload Matcha plugin to running server
[working-directory: 'plugins/native/matcha']
upload-matcha-plugin: build-plugin-native-matcha
    @echo "Uploading Matcha plugin to server..."
    @curl -X POST -F "plugin=@{{plugins_target_dir}}/release/libmatcha.so" \
        http://127.0.0.1:4545/api/v1/plugins

# Download SenseVoice models
download-sensevoice-models:
    @echo "Downloading SenseVoice models..."
    @mkdir -p models
    @if [ -f models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2025-09-09.tar.bz2 ]; then \
        echo "✓ SenseVoice archive already exists"; \
    else \
        echo "Downloading sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2025-09-09.tar.bz2 (~158MB)..." && \
        curl -L -o models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2025-09-09.tar.bz2 \
            https://huggingface.co/streamkit/sensevoice-models/resolve/main/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2025-09-09.tar.bz2 && \
        echo "✓ SenseVoice archive downloaded"; \
    fi
    @if [ -d models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2025-09-09 ]; then \
        echo "✓ SenseVoice models already extracted"; \
    else \
        echo "Extracting models..." && \
        cd models && tar xf sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2025-09-09.tar.bz2 && \
        echo "✓ SenseVoice models ready at models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2025-09-09 (multilingual: zh, en, ja, ko, yue)"; \
    fi

# Setup SenseVoice (install dependencies + download models)
setup-sensevoice: install-sherpa-onnx download-sensevoice-models download-silero-vad
    @echo "✓ SenseVoice STT setup complete!"

# Build native SenseVoice STT plugin
[working-directory: 'plugins/native/sensevoice']
build-plugin-native-sensevoice:
    @echo "Building native SenseVoice STT plugin..."
    @CARGO_TARGET_DIR={{plugins_target_dir}} cargo build --release

# Upload SenseVoice plugin to running server
[working-directory: 'plugins/native/sensevoice']
upload-sensevoice-plugin: build-plugin-native-sensevoice
    @echo "Uploading SenseVoice plugin to server..."
    @curl -X POST -F "plugin=@{{plugins_target_dir}}/release/libsensevoice.so" \
        http://127.0.0.1:4545/api/v1/plugins

# Build native Parakeet TDT STT plugin
[working-directory: 'plugins/native/parakeet']
build-plugin-native-parakeet:
    @echo "Building native Parakeet TDT STT plugin..."
    @CARGO_TARGET_DIR={{plugins_target_dir}} cargo build --release

# Upload Parakeet plugin to running server
[working-directory: 'plugins/native/parakeet']
upload-parakeet-plugin: build-plugin-native-parakeet
    @echo "Uploading Parakeet plugin to server..."
    @curl -X POST -F "plugin=@{{plugins_target_dir}}/release/libparakeet.so" \
        http://127.0.0.1:4545/api/v1/plugins

# Download Parakeet TDT v3 models (25 languages, INT8)
download-parakeet-models:
    @echo "Downloading Parakeet TDT v3 models (~671MB)..."
    @mkdir -p models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8
    @HF_BASE="https://huggingface.co/streamkit/parakeet-models/resolve/main" && \
    MODEL_DIR="models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8" && \
    for f in encoder.int8.onnx decoder.int8.onnx joiner.int8.onnx tokens.txt; do \
        if [ -f "$MODEL_DIR/$f" ]; then \
            echo "✓ $f already exists"; \
        else \
            echo "Downloading $f..." && \
            curl -L -o "$MODEL_DIR/$f" "$HF_BASE/$f" || exit 1; \
        fi; \
    done && \
    echo "✓ Parakeet TDT v3 models ready at $MODEL_DIR (25 languages)"

# Setup Parakeet (install dependencies + download models)
setup-parakeet: install-sherpa-onnx download-parakeet-models download-silero-vad
    @echo "✓ Parakeet TDT STT setup complete!"

# Download pre-converted NLLB models from Hugging Face
download-nllb-models:
    @echo "Downloading pre-converted NLLB-200 models from Hugging Face..."
    @mkdir -p models
    @if [ -f models/nllb-200-distilled-600M-ct2-int8.tar.bz2 ]; then \
        echo "✓ NLLB archive already exists"; \
    else \
        echo "Downloading nllb-200-distilled-600M-ct2-int8.tar.bz2 (~1.1GB)..." && \
        curl -L -o models/nllb-200-distilled-600M-ct2-int8.tar.bz2 \
            https://huggingface.co/streamkit/nllb-models/resolve/main/nllb-200-distilled-600M-ct2-int8.tar.bz2 && \
        echo "✓ NLLB archive downloaded"; \
    fi
    @if [ -d models/nllb-200-distilled-600M-ct2-int8 ]; then \
        echo "✓ NLLB model already extracted"; \
    else \
        echo "Extracting models..." && \
        cd models && tar xf nllb-200-distilled-600M-ct2-int8.tar.bz2 && \
        echo "✓ NLLB model ready at models/nllb-200-distilled-600M-ct2-int8 (supports 200 languages)"; \
    fi

# Download Spanish Piper TTS model
download-piper-spanish:
    @./plugins/native/piper/download-piper-spanish.sh

# Setup NLLB (download and convert models)
setup-nllb: download-nllb-models
    @echo "✓ NLLB translation setup complete!"
    @echo ""
    @echo "⚠️  LICENSE WARNING: NLLB-200 models are CC-BY-NC-4.0 (non-commercial only)"
    @echo "   For commercial use, consider Opus-MT models (Apache 2.0)"

# Build native NLLB translation plugin
[working-directory: 'plugins/native/nllb']
build-plugin-native-nllb:
    @echo "Building native NLLB translation plugin..."
    @CARGO_TARGET_DIR={{plugins_target_dir}} cargo build --release

# Upload NLLB plugin to running server
[working-directory: 'plugins/native/nllb']
upload-nllb-plugin: build-plugin-native-nllb
    @echo "Uploading NLLB plugin to server..."
    @curl -X POST -F "plugin=@{{plugins_target_dir}}/release/libnllb.so" \
        http://127.0.0.1:4545/api/v1/plugins

# Download ten-vad models
download-tenvad-models:
    @echo "Downloading ten-vad models..."
    @mkdir -p models
    @if [ -f models/ten-vad.onnx ]; then \
        echo "✓ ten-vad model already exists at models/ten-vad.onnx"; \
    else \
        echo "Downloading ten-vad.onnx (~324KB)..." && \
        curl -L -o models/ten-vad.onnx \
            https://huggingface.co/streamkit/vad-models/resolve/main/ten-vad.onnx && \
        echo "✓ ten-vad model downloaded to models/ten-vad.onnx ($(du -h models/ten-vad.onnx | cut -f1))"; \
    fi

# Download all models (for Docker deployment)
# NOTE: NLLB is CC-BY-NC-4.0 (non-commercial only) - skipped by default
download-models: download-whisper-models download-silero-vad download-kokoro-models download-piper-models download-matcha-models download-sensevoice-models download-tenvad-models
    @echo ""
    @echo "✓ All models downloaded to ./models/"
    @echo ""
    @echo "Optional: To download NLLB translation models (CC-BY-NC-4.0 license - non-commercial only):"
    @echo "  just download-nllb-models"
    @echo ""
    @echo "Optional: To download Pocket TTS models (gated; requires HF_TOKEN):"
    @echo "  just download-pocket-tts-models"
    @echo ""
    @echo "Optional: To download Parakeet TDT models (~660MB, CC-BY-4.0):"
    @echo "  just download-parakeet-models"
    @echo ""
    @du -sh models/

# Setup VAD (install dependencies + download models)
setup-vad: install-sherpa-onnx download-tenvad-models
    @echo "✓ VAD setup complete!"

# Build native VAD plugin
[working-directory: 'plugins/native/vad']
build-plugin-native-vad:
    @echo "Building native VAD plugin..."
    @CARGO_TARGET_DIR={{plugins_target_dir}} cargo build --release

# Upload VAD plugin to running server
[working-directory: 'plugins/native/vad']
upload-vad-plugin: build-plugin-native-vad
    @echo "Uploading VAD plugin to server..."
    @curl -X POST -F "plugin=@{{plugins_target_dir}}/release/libvad.so" \
        http://127.0.0.1:4545/api/v1/plugins

# Download Helsinki-NLP OPUS-MT models for translation (pre-packaged from StreamKit HuggingFace)
download-helsinki-models:
    @echo "Downloading Helsinki OPUS-MT models..."
    @mkdir -p models
    @if [ -f models/opus-mt-en-es.tar.bz2 ]; then \
        echo "✓ opus-mt-en-es archive already exists"; \
    else \
        echo "Downloading opus-mt-en-es.tar.bz2 (~272MB)..." && \
        curl -L -o models/opus-mt-en-es.tar.bz2 \
            https://huggingface.co/streamkit/helsinki-models/resolve/main/opus-mt-en-es.tar.bz2 && \
        echo "✓ opus-mt-en-es archive downloaded"; \
    fi
    @if [ -f models/opus-mt-es-en.tar.bz2 ]; then \
        echo "✓ opus-mt-es-en archive already exists"; \
    else \
        echo "Downloading opus-mt-es-en.tar.bz2 (~272MB)..." && \
        curl -L -o models/opus-mt-es-en.tar.bz2 \
            https://huggingface.co/streamkit/helsinki-models/resolve/main/opus-mt-es-en.tar.bz2 && \
        echo "✓ opus-mt-es-en archive downloaded"; \
    fi
    @if [ -d models/opus-mt-en-es ]; then \
        echo "✓ opus-mt-en-es model already extracted"; \
    else \
        echo "Extracting opus-mt-en-es..." && \
        cd models && tar xf opus-mt-en-es.tar.bz2 && \
        echo "✓ opus-mt-en-es extracted"; \
    fi
    @if [ -d models/opus-mt-es-en ]; then \
        echo "✓ opus-mt-es-en model already extracted"; \
    else \
        echo "Extracting opus-mt-es-en..." && \
        cd models && tar xf opus-mt-es-en.tar.bz2 && \
        echo "✓ opus-mt-es-en extracted"; \
    fi
    @echo "✓ Helsinki OPUS-MT models ready (Apache 2.0 license)"

# Download Helsinki models from original source (requires Python dependencies)
download-helsinki-models-source:
    @echo "⚠️  This requires Python with transformers and tokenizers installed."
    @echo "⚠️  Install with: pip3 install --user transformers sentencepiece safetensors torch tokenizers"
    @echo ""
    @python3 plugins/native/helsinki/download-models.py

# Setup Helsinki translation (download models)
setup-helsinki: download-helsinki-models
    @echo "✓ Helsinki translation setup complete!"
    @echo ""
    @echo "✓ LICENSE: Apache 2.0 - suitable for commercial use"

# Build native Helsinki translation plugin
[working-directory: 'plugins/native/helsinki']
build-plugin-native-helsinki:
    @echo "Building native Helsinki translation plugin..."
    @CARGO_TARGET_DIR={{plugins_target_dir}} cargo build --release

# Build Helsinki plugin with CUDA support
[working-directory: 'plugins/native/helsinki']
build-plugin-native-helsinki-cuda:
    @echo "Building native Helsinki translation plugin with CUDA..."
    @CARGO_TARGET_DIR={{plugins_target_dir}} cargo build --release --features cuda

# Upload Helsinki plugin to running server
[working-directory: 'plugins/native/helsinki']
upload-helsinki-plugin: build-plugin-native-helsinki
    @echo "Uploading Helsinki plugin to server..."
    @curl -X POST -F "plugin=@{{plugins_target_dir}}/release/libhelsinki.so" \
        http://127.0.0.1:4545/api/v1/plugins

# Download Supertonic TTS models
download-supertonic-models:
    @echo "Downloading Supertonic TTS models..."
    @mkdir -p models
    @if [ -f models/supertonic-v2-onnx.tar.bz2 ]; then \
        echo "✓ Supertonic archive already exists at models/supertonic-v2-onnx.tar.bz2"; \
    else \
        echo "Downloading supertonic-v2-onnx.tar.bz2..." && \
        curl -L -o models/supertonic-v2-onnx.tar.bz2 \
            https://huggingface.co/streamkit/supertonic-models/resolve/main/supertonic-v2-onnx.tar.bz2 && \
        echo "✓ Supertonic archive downloaded"; \
    fi
    @if [ -d models/supertonic-v2-onnx ]; then \
        echo "✓ Supertonic models already extracted at models/supertonic-v2-onnx"; \
    else \
        echo "Extracting models..." && \
        cd models && tar xf supertonic-v2-onnx.tar.bz2 && \
        echo "✓ Supertonic v2 models ready at models/supertonic-v2-onnx (5 languages, 10 voices)"; \
    fi

# Setup Supertonic TTS (download models)
setup-supertonic: download-supertonic-models
    @echo "✓ Supertonic TTS setup complete!"

# Build native Supertonic TTS plugin
[working-directory: 'plugins/native/supertonic']
build-plugin-native-supertonic:
    @echo "Building native Supertonic TTS plugin..."
    @CARGO_TARGET_DIR={{plugins_target_dir}} cargo build --release

# Upload Supertonic plugin to running server
[working-directory: 'plugins/native/supertonic']
upload-supertonic-plugin: build-plugin-native-supertonic
    @echo "Uploading Supertonic plugin to server..."
    @curl -X POST -F "plugin=@{{plugins_target_dir}}/release/libsupertonic.so" \
        http://127.0.0.1:4545/api/v1/plugins

# Build native Slint UI plugin
[working-directory: 'plugins/native/slint']
build-plugin-native-slint:
    @echo "Building native Slint UI plugin..."
    @CARGO_TARGET_DIR={{plugins_target_dir}} cargo build --release

# Upload Slint plugin to running server
[working-directory: 'plugins/native/slint']
upload-slint-plugin: build-plugin-native-slint
    @echo "Uploading Slint plugin to server..."
    @curl -X POST -F "plugin=@{{plugins_target_dir}}/release/libslint.so" \
        http://127.0.0.1:4545/api/v1/plugins

# Build AAC encoder native plugin (requires libfdk-aac-dev)
[working-directory: 'plugins/native/aac-encoder']
build-plugin-native-aac-encoder:
    @echo "Building native AAC encoder plugin..."
    @CARGO_TARGET_DIR={{plugins_target_dir}} cargo build --release

# Build specific native plugin by name
build-plugin-native name:
    @just build-plugin-native-{{name}}

# Build and install a single native plugin to .plugins/native/<name>/ (idempotent).
# Creates a bundle directory with the compiled library and plugin.yml manifest.
# The plugin loader scans subdirectories, so this layout is discovered at startup.
# Usage: just install-plugin slint
#        just install-plugin pocket-tts
install-plugin name: (build-plugin-native name)
    #!/usr/bin/env bash
    set -euo pipefail
    PLUGINS_TARGET="{{plugins_target_dir}}"
    BUNDLE_DIR=".plugins/native/{{name}}"
    mkdir -p "$BUNDLE_DIR"

    # Rust library names use underscores for hyphens.
    LIB_STEM="lib$(echo '{{name}}' | tr '-' '_')"

    SO_FILE=""
    for ext in so dylib dll; do
        candidate="$PLUGINS_TARGET/release/${LIB_STEM}.${ext}"
        if [[ -f "$candidate" ]]; then
            SO_FILE="$candidate"
            break
        fi
    done

    if [[ -z "$SO_FILE" ]]; then
        echo "❌ Built library not found (expected ${LIB_STEM}.{so,dylib,dll} in $PLUGINS_TARGET/release/)"
        exit 1
    fi

    cp -f "$SO_FILE" "$BUNDLE_DIR/"
    echo "✓ Copied $(basename "$SO_FILE") → $BUNDLE_DIR/"

    # Copy plugin.yml into the bundle so asset types are discovered at startup
    # (see plugin_assets::read_local_plugin_manifest).
    if [[ -f "plugins/native/{{name}}/plugin.yml" ]]; then
        cp -f "plugins/native/{{name}}/plugin.yml" "$BUNDLE_DIR/plugin.yml"
        echo "✓ Copied plugin.yml → $BUNDLE_DIR/plugin.yml"
    fi

# Build all native plugin examples
build-plugins-native: build-plugin-native-gain build-plugin-native-whisper build-plugin-native-kokoro build-plugin-native-piper build-plugin-native-matcha build-plugin-native-pocket-tts build-plugin-native-sensevoice build-plugin-native-nllb build-plugin-native-vad build-plugin-native-helsinki build-plugin-native-supertonic build-plugin-native-slint build-plugin-native-aac-encoder build-plugin-native-parakeet

## Combined

# Build all plugin examples (both WASM and native)
build-plugins: build-plugins-wasm build-plugins-native

# Copy built plugins to the runtime plugins directory
install-plugins: build-plugins
    @just copy-plugins

# Copy built plugins to the runtime plugins directory (does not build).
copy-plugins: copy-plugins-wasm copy-plugins-native
    @echo "✓ Plugins copied to .plugins/"

copy-plugins-wasm:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p .plugins/wasm
    cp examples/plugins/gain-wasm-rust/target/wasm32-wasip1/release/gain_plugin.wasm .plugins/wasm/ 2>/dev/null || true
    cp examples/plugins/gain-wasm-go/build/gain_plugin_go.wasm .plugins/wasm/ 2>/dev/null || true
    cp examples/plugins/gain-wasm-c/build/gain_plugin_c.wasm .plugins/wasm/ 2>/dev/null || true
    echo "✓ WASM plugins copied to .plugins/wasm/"

copy-plugins-native:
    #!/usr/bin/env bash
    set -euo pipefail
    shopt -s nullglob
    mkdir -p .plugins/native

    PLUGINS_TARGET="{{plugins_target_dir}}"

    # Helper: copy a built plugin into the directory-per-plugin layout.
    #   copy_plugin <plugin-id> <lib-stem> <source-dir>
    # Creates .plugins/native/<plugin-id>/ with the .so and plugin.yml.
    copy_plugin() {
        local id="$1" stem="$2" src_dir="$3"
        local dest=".plugins/native/$id"
        mkdir -p "$dest"

        for f in \
            "$src_dir"/release/lib"$stem".so \
            "$src_dir"/release/lib"$stem".so.* \
            "$src_dir"/release/lib"$stem".dylib \
            "$src_dir"/release/"$stem".dll
        do
            if [[ -f "$f" ]]; then
                cp -f "$f" "$dest/"
            fi
        done

        # Copy plugin.yml into the bundle directory so asset types can be
        # discovered at runtime (see plugin_assets::read_local_plugin_manifest).
        if [[ -f "plugins/native/$id/plugin.yml" ]]; then
            cp -f "plugins/native/$id/plugin.yml" "$dest/plugin.yml"
        fi
    }

    # Examples (gain-native has its own target dir, not shared)
    copy_plugin "gain-native" "gain_plugin_native" "examples/plugins/gain-native/target"

    # Official native plugins (shared target dir).
    # For most plugins the lib stem matches the plugin id.
    for name in whisper kokoro piper matcha vad sensevoice nllb helsinki supertonic slint parakeet; do
        copy_plugin "$name" "$name" "$PLUGINS_TARGET"
    done

    # Plugins whose lib stem differs from the plugin id.
    copy_plugin "pocket-tts"   "pocket_tts"   "$PLUGINS_TARGET"
    copy_plugin "aac-encoder"  "aac_encoder"  "$PLUGINS_TARGET"

    echo "✓ Native plugins copied to .plugins/native/"

# --- License Headers (REUSE) ---

# Check REUSE compliance (all files have proper license headers)
check-license-headers:
    @echo "Checking REUSE compliance..."
    @reuse --no-multiprocessing lint

# Automatically add missing license headers to source files
fix-license-headers:
    @echo "Adding missing license headers..."
    @echo "Note: This will add headers to files without them. Generated files are handled by REUSE.toml"
    @find . -type f \( -name "*.rs" -o -name "*.ts" -o -name "*.tsx" -o -name "*.js" -o -name "*.jsx" \) \
        ! -path "*/target/*" \
        ! -path "*/node_modules/*" \
        ! -path "*/dist/*" \
        ! -path "*/build/*" \
        ! -path "*/.next/*" \
        ! -path "*/generated/*" \
        ! -path "*/bindings/*" \
        -exec sh -c 'if ! head -n 2 "{}" | grep -q "SPDX-License-Identifier"; then reuse annotate --copyright="© 2025 StreamKit Contributors" --license="MPL-2.0" --exclude-year --skip-existing "{}"; fi' \;
    @echo "✓ Done. Run 'just check-license-headers' to verify."

# Generate third-party license report (Rust crates) for redistribution
gen-third-party-licenses:
    @echo "Generating THIRD_PARTY_LICENSES.txt..."
    @cargo about generate --workspace --locked --offline tools/licenses/third-party-licenses.hbs --output-file THIRD_PARTY_LICENSES.txt
    @echo "Note: cargo-about may log 'GPL-2.0' (deprecated SPDX id) while scanning; output is still generated."
    # Avoid REUSE falsely interpreting SPDX identifiers inside embedded license texts.
    @sed -i -e 's/^SPDX-License-Identifier:/SPDX License Identifier:/' -e 's/^SPDX-FileCopyrightText:/SPDX Copyright:/' THIRD_PARTY_LICENSES.txt

# --- Release & Packaging ---

# Generate changelog for a given version (e.g., just changelog v0.2.0)
changelog version:
    @echo "Generating changelog for {{version}}..."
    @git cliff --tag {{version}} > CHANGELOG.md
    @echo "✓ CHANGELOG.md updated for {{version}}"
    @echo "  Review the changes and commit with:"
    @echo "  git add CHANGELOG.md"
    @echo "  git commit -m 'chore: update changelog for {{version}}'"

# Preview unreleased changes that would go in the next changelog
changelog-unreleased:
    @echo "Unreleased changes:"
    @git cliff --unreleased --strip header

# Preview changelog between two tags (e.g., just changelog-range v0.1.0 v0.2.0)
changelog-range from to:
    @echo "Changelog from {{from}} to {{to}}:"
    @git cliff {{from}}..{{to}}

# Build release artifacts (binaries + tarball) for local testing
# This mimics what the GitHub release workflow does
package version="dev": build-ui
    @echo "Building release package ({{version}})..."
    @cargo build -p streamkit-server --bin skit --release --features "moq"
    @cargo build -p streamkit-client --bin skit-cli --release
    @echo "Stripping binaries..."
    @strip target/release/skit
    @strip target/release/skit-cli
    @echo "Creating release directory..."
    @rm -rf target/streamkit
    @mkdir -p target/streamkit/plugins/{wasm,native}
    @cp target/release/skit target/streamkit/
    @cp target/release/skit-cli target/streamkit/
    @cp LICENSE target/streamkit/
    @cp README.md target/streamkit/
    @cp NOTICE target/streamkit/
    @cp THIRD_PARTY_LICENSES.txt target/streamkit/
    @cp -r LICENSES target/streamkit/
    @echo "Creating tarball..."
    @cd target && tar -czf streamkit-{{version}}-linux-x64.tar.gz streamkit/
    @echo "Generating checksum..."
    @cd target && sha256sum streamkit-{{version}}-linux-x64.tar.gz > streamkit-{{version}}-linux-x64.tar.gz.sha256
    @echo "✓ Release package created:"
    @echo "  target/streamkit-{{version}}-linux-x64.tar.gz"
    @echo "  target/streamkit-{{version}}-linux-x64.tar.gz.sha256"
    @echo ""
    @echo "  Extract and test with:"
    @echo "  cd target && tar -xzf streamkit-{{version}}-linux-x64.tar.gz"
    @echo "  ./streamkit/skit --version"

# Verify release package by extracting and running basic checks
verify-package version="dev":
    @echo "Verifying release package for {{version}}..."
    @if [ ! -f "target/streamkit-{{version}}-linux-x64.tar.gz" ]; then \
        echo "❌ Package not found. Run: just package {{version}}"; \
        exit 1; \
    fi
    @echo "Checking tarball integrity..."
    @cd target && sha256sum -c streamkit-{{version}}-linux-x64.tar.gz.sha256
    @echo "Extracting package..."
    @rm -rf target/streamkit-test
    @mkdir -p target/streamkit-test
    @cd target/streamkit-test && tar -xzf ../streamkit-{{version}}-linux-x64.tar.gz
    @echo "Checking binaries..."
    @target/streamkit-test/streamkit/skit --version
    @target/streamkit-test/streamkit/skit-cli --version
    @echo "Checking file structure..."
    @ls -lh target/streamkit-test/streamkit/
    @echo "✓ Package verification successful!"
    @rm -rf target/streamkit-test

# Run pre-release checks (lint, test, license)
pre-release:
    @echo "Running pre-release checks..."
    @echo ""
    @echo "→ Checking license headers..."
    @just check-license-headers
    @echo ""
    @echo "→ Running linters..."
    @just lint-skit
    @just lint-ui
    @echo ""
    @echo "→ Running tests..."
    @just test
    @echo ""
    @echo "✓ All pre-release checks passed!"
    @echo ""
    @echo "Next steps for releasing:"
    @echo "  1. Generate changelog: just changelog v0.x.x"
    @echo "  2. Review and commit changelog"
    @echo "  3. Create and push tag: git tag v0.x.x -m 'Release v0.x.x' && git push origin v0.x.x"
    @echo "  4. GitHub Actions will build and create the release automatically"

# Dry-run publish a crate to crates.io (doesn't actually publish)
publish-dry-run crate:
    @echo "Dry-run publishing {{crate}} to crates.io..."
    @cargo publish -p {{crate}} --dry-run

# Publish a crate to crates.io (requires confirmation)
publish crate:
    @echo "⚠️  Publishing {{crate}} to crates.io..."
    @echo "This will make the package publicly available!"
    @read -p "Continue? [y/N] " -n 1 -r; \
    echo; \
    if [[ ! $$REPLY =~ ^[Yy]$$ ]]; then \
        echo "❌ Aborted"; \
        exit 1; \
    fi
    @cargo publish -p {{crate}}
    @echo "✓ {{crate}} published successfully!"
    @echo "  View at: https://crates.io/crates/{{crate}}"

# Publish critical crates in dependency order (dry-run by default)
publish-sdk mode="dry-run":
    @echo "Publishing SDK crates ({{mode}} mode)..."
    @if [ "{{mode}}" = "dry-run" ]; then \
        echo "→ streamkit-core..."; \
        just publish-dry-run streamkit-core; \
        echo ""; \
        echo "→ streamkit-plugin-sdk-wasm..."; \
        just publish-dry-run streamkit-plugin-sdk-wasm; \
        echo ""; \
        echo "→ streamkit-plugin-sdk-native..."; \
        just publish-dry-run streamkit-plugin-sdk-native; \
        echo ""; \
        echo "→ streamkit-api..."; \
        just publish-dry-run streamkit-api; \
        echo ""; \
        echo "✓ Dry-run complete! To actually publish, run:"; \
        echo "  just publish-sdk publish"; \
    else \
        echo "→ streamkit-core..."; \
        just publish streamkit-core; \
        echo ""; \
        echo "→ streamkit-plugin-sdk-wasm..."; \
        just publish streamkit-plugin-sdk-wasm; \
        echo ""; \
        echo "→ streamkit-plugin-sdk-native..."; \
        just publish streamkit-plugin-sdk-native; \
        echo ""; \
        echo "→ streamkit-api..."; \
        just publish streamkit-api; \
        echo ""; \
        echo "✓ All SDK crates published!"; \
    fi

# Show current versions of all publishable crates
show-versions:
    @echo "Crate versions:"
    @echo ""
    @echo "Core crates:"
    @grep '^version' crates/core/Cargo.toml | head -1 | awk '{print "  streamkit-core:               " $$3}'
    @grep '^version' crates/api/Cargo.toml | head -1 | awk '{print "  streamkit-api:                " $$3}'
    @if [ -f "crates/pipeline/Cargo.toml" ]; then \
        grep '^version' crates/pipeline/Cargo.toml | head -1 | awk '{print "  streamkit-pipeline:           " $$3}'; \
    fi
    @grep '^version' crates/engine/Cargo.toml | head -1 | awk '{print "  streamkit-engine:             " $$3}'
    @grep '^version' crates/nodes/Cargo.toml | head -1 | awk '{print "  streamkit-nodes:              " $$3}'
    @echo ""
    @echo "Plugin SDKs:"
    @grep '^version' sdks/plugin-sdk/wasm/rust/Cargo.toml | head -1 | awk '{print "  streamkit-plugin-sdk-wasm:    " $$3}'
    @grep '^version' sdks/plugin-sdk/native/Cargo.toml | head -1 | awk '{print "  streamkit-plugin-sdk-native:  " $$3}'
    @echo ""
    @echo "Plugin Runtimes:"
    @grep '^version' crates/plugin-wasm/Cargo.toml | head -1 | awk '{print "  streamkit-plugin-wasm:        " $$3}'
    @grep '^version' crates/plugin-native/Cargo.toml | head -1 | awk '{print "  streamkit-plugin-native:      " $$3}'
    @echo ""
    @echo "Binaries:"
    @grep '^version' apps/skit/Cargo.toml | head -1 | awk '{print "  streamkit-server (skit):      " $$3}'
    @grep '^version' apps/skit-cli/Cargo.toml | head -1 | awk '{print "  streamkit-client (skit-cli):  " $$3}'

# --- E2E Tests ---

# Pinned moq-relay commit for e2e relay tests
moq_relay_commit := "8b1d09c7a53bad1c573bff648c593cb1903c96f7"
moq_relay_repo := "https://github.com/moq-dev/moq.git"

# Build moq-relay binary for e2e relay tests
build-moq-relay:
    #!/usr/bin/env bash
    set -euo pipefail
    RELAY_BIN="target/moq-relay/moq-relay"
    COMMIT_FILE="target/moq-relay/.commit"
    if [ -f "$RELAY_BIN" ] && [ -f "$COMMIT_FILE" ] && [ "$(cat "$COMMIT_FILE")" = "{{moq_relay_commit}}" ]; then
        echo "moq-relay binary already exists at $RELAY_BIN for commit {{moq_relay_commit}} — skipping build"
        exit 0
    fi
    CLONE_DIR="target/moq-relay-src"
    echo "Cloning moq-dev/moq at {{moq_relay_commit}}..."
    if [ ! -d "$CLONE_DIR/.git" ]; then
        git clone --depth 1 {{moq_relay_repo}} "$CLONE_DIR"
    fi
    cd "$CLONE_DIR"
    git fetch origin {{moq_relay_commit}} --depth 1
    git checkout {{moq_relay_commit}}
    echo "Building moq-relay (release)..."
    cargo build -p moq-relay --release
    cd ../..
    mkdir -p target/moq-relay
    cp "$CLONE_DIR/target/release/moq-relay" "$RELAY_BIN"
    echo "{{moq_relay_commit}}" > "$COMMIT_FILE"
    echo "moq-relay binary ready at $RELAY_BIN"

# Install E2E test dependencies
[working-directory: 'e2e']
install-e2e:
    @echo "Installing E2E dependencies..."
    @bun install

# Lint E2E (TypeScript + formatting)
lint-e2e: install-e2e
    @echo "Linting E2E..."
    @cd e2e && bun run lint

# Install Playwright browsers
[working-directory: 'e2e']
install-playwright: install-e2e
    @echo "Installing Playwright browsers..."
    @bunx playwright install chromium

# Run E2E tests (builds UI and skit if needed)
e2e: build-ui install-e2e
    @echo "Building skit (debug)..."
    @cargo build -p streamkit-server --bin skit
    @echo "Running E2E tests..."
    @cd e2e && bun run test

# Run E2E tests with headed browser
e2e-headed: build-ui install-e2e
    @cargo build -p streamkit-server --bin skit
    @cd e2e && bun run test:headed

# Run E2E tests with built-in auth enabled
e2e-auth: build-ui install-e2e
    @echo "Building skit (debug)..."
    @cargo build -p streamkit-server --bin skit
    @echo "Running E2E tests (auth enabled)..."
    @cd e2e && E2E_AUTH=1 bun run test

# Run E2E tests with built-in auth enabled + headed browser
e2e-auth-headed: build-ui install-e2e
    @cargo build -p streamkit-server --bin skit
    @cd e2e && E2E_AUTH=1 bun run test:headed

# Run E2E against external server (optional filter greps test names)
e2e-external url filter='':
    @echo "Running E2E tests against {{url}}..."
    @cd e2e && E2E_BASE_URL={{url}} bun run test:only {{ if filter != "" { "--grep '" + filter + "'" } else { "" } }}

# Show E2E test report
[working-directory: 'e2e']
e2e-report:
    @bunx playwright show-report
