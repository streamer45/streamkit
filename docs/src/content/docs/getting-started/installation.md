---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Installation
description: Detailed installation instructions for StreamKit
---

This guide covers the supported ways to install and run StreamKit.

## System Requirements

StreamKit currently supports **Linux x86_64**. Real-world resource needs depend heavily on which pipelines/plugins you run (especially ML models).

> [!NOTE]
> Official Docker images are published for `linux/amd64` (x86_64).

## Docker (recommended)

Use the prebuilt images and follow the full guide:

- [Docker Deployment](/deployment/docker/)
- [GPU Setup](/deployment/gpu/)

## GitHub Release + systemd (Linux)

If you want a native host install without containers, you can run the released `skit` binary via `systemd`:

- [systemd Deployment](/deployment/systemd/)

## Build from Source

### Prerequisites

Required:

- Rust toolchain (the repo is pinned via `rust-toolchain.toml`)
- [Just](https://github.com/casey/just) task runner (`cargo install just`)
- [Bun](https://bun.sh) (`bun` in `$PATH`) to build the embedded web UI (`ui/dist`)
- System libraries (Ubuntu/Debian): `sudo apt install libopus-dev cmake pkg-config libssl-dev`

Optional:

- `cargo-watch` (`cargo install cargo-watch`) for `just dev`
- `cargo-deny` (`cargo install cargo-deny`) for license checks in `just lint`
- `reuse` (`pip3 install --user reuse`) for SPDX license header checks in `just lint` (note: the apt package is too old)
- `clang` and `libclang-dev` (`sudo apt install clang libclang-dev`) for building native ML plugins (e.g. whisper, sensevoice)
- `libvpx-dev` + `pkg-config` (`sudo apt install libvpx-dev pkg-config`) if building with `--features video` or using VP9 sample pipelines
- `cmake` + `nasm` + C compiler if building with `--features svt_av1_static` (SVT-AV1 encoder); see [`crates/nodes/SVT_AV1.md`](https://github.com/streamer45/streamkit/blob/main/crates/nodes/SVT_AV1.md) for details
- `libdav1d-dev` if building with `--features dav1d` (C dav1d AV1 decoder); the pure-Rust rav1d decoder (`--features av1`) requires no extra deps

### Build Steps

```bash
git clone https://github.com/streamer45/streamkit.git
cd streamkit

# Build the embedded web UI first (creates ui/dist/, required by RustEmbed)
just build-ui

# Build the server (release)
just build-skit

# Verify installation
./target/release/skit --version
```

To build plugins locally and copy them into the default runtime directory (`.plugins/`), run:

```bash
just install-plugins
```

> [!NOTE]
> Some native ML plugins require additional system dependencies to build (e.g. sherpa-onnx). If you only need the core server, skip plugin builds.

For hot-reload development, install `cargo-watch` before running `just dev`:

```bash
cargo install cargo-watch
just build-ui # one-time preflight so the server can compile
just dev
```

## Configuration

StreamKit uses a TOML configuration file. By default `skit` reads `skit.toml` (or uses defaults if missing).

> [!CAUTION]
> StreamKit ships with built-in authentication. If you expose the server beyond localhost, keep auth enabled (default in `auth.mode = "auto"`) and follow the [Authentication](/guides/authentication/) and [Security](/guides/security/) guides.

```toml
[server]
address = "127.0.0.1:4545"

[plugins]
directory = ".plugins"

[resources]
keep_models_loaded = true
max_memory_mb = 8192
```

If you bind to a non-loopback address (e.g. `0.0.0.0:4545`), StreamKit enables built-in auth by default (`[auth].mode = "auto"`). If you disable built-in auth, you must configure a trusted role header (`[permissions].role_header`) behind an auth layer, or explicitly opt out with `[permissions].allow_insecure_no_auth = true` (unsafe).

Environment variables override config file settings:

```bash
export SK_SERVER__ADDRESS=127.0.0.1:8080
export SK_PLUGINS__DIRECTORY=/opt/plugins
```

## Verify Installation

```bash
# Basic server check (also used by the UI)
curl http://localhost:4545/api/v1/config

# List available node kinds + schemas
curl http://localhost:4545/api/v1/schema/nodes

# List packet types
curl http://localhost:4545/api/v1/schema/packets

# Open the web UI in your browser
echo "http://localhost:4545"
```

## Next Steps

- [Quick Start](/getting-started/quick-start/) - Create your first pipeline
- [Docker Deployment](/deployment/docker/) - Production Docker setup
- [GPU Setup](/deployment/gpu/) - Detailed GPU configuration
