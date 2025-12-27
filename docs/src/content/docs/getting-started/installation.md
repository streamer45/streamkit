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
- `just` (`cargo install just`)
- Bun (`bun` in `$PATH`) to build the embedded web UI (`ui/dist`)

Optional:

- `cargo-watch` (`cargo install cargo-watch`) for `just dev`

### Build Steps

```bash
git clone https://github.com/streamer45/streamkit.git
cd streamkit

# Build the embedded web UI (creates ui/dist/)
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

## Configuration

StreamKit uses a TOML configuration file. By default `skit` reads `skit.toml` (or uses defaults if missing).

> [!CAUTION]
> StreamKit does not currently implement authentication. If you expose the server beyond localhost, put it behind an authenticating reverse proxy (nginx/Caddy/etc) and configure a trusted role header.

```toml
[server]
address = "127.0.0.1:4545"

[plugins]
directory = ".plugins"

[resources]
keep_models_loaded = true
max_memory_mb = 8192
```

If you bind to a non-loopback address (e.g. `0.0.0.0:4545`), you must either configure a trusted role header (`[permissions].role_header`) behind an auth layer, or explicitly opt out with `[permissions].allow_insecure_no_auth = true` (unsafe).

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
