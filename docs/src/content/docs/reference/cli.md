---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: CLI Reference
description: skit command-line interface
---

The StreamKit server binary is `skit` (crate: `streamkit-server`).

## Installation

### Option 1: GitHub Release tarball (recommended)

Each GitHub Release includes a Linux x86_64 tarball containing both binaries:

- `skit` (server)
- `skit-cli` (client)

Install steps:

```bash
TAG=v0.1.0 # replace with the latest release tag
curl -LO https://github.com/streamer45/streamkit/releases/download/${TAG}/streamkit-${TAG}-linux-x64.tar.gz
curl -LO https://github.com/streamer45/streamkit/releases/download/${TAG}/streamkit-${TAG}-linux-x64.tar.gz.sha256
sha256sum -c streamkit-${TAG}-linux-x64.tar.gz.sha256

tar -xzf streamkit-${TAG}-linux-x64.tar.gz
sudo install -m 0755 streamkit-${TAG}/skit /usr/local/bin/skit
sudo install -m 0755 streamkit-${TAG}/skit-cli /usr/local/bin/skit-cli
```

### Option 2: Build from source (repo)

From the repo root:

```bash
just build-skit
just build-skit-cli
```

## Global Flags

| Flag | Short | Default | Description |
|------|-------|---------|-------------|
| `--config` | `-c` | `skit.toml` | Path to the configuration file |

If the config file doesn't exist, the server uses built-in defaults and logs a warning.

## Default Behavior

Running `skit` without a subcommand defaults to `skit serve`:

```bash
skit                    # Same as: skit serve
skit -c custom.toml     # Same as: skit -c custom.toml serve
```

## Commands

### `skit serve`

Starts the HTTP server (UI + API).

```bash
skit serve
skit -c skit.toml serve
```

### `skit config default`

Prints the default config to stdout:

```bash
skit config default > skit.toml
```

### `skit config schema`

Prints the JSON schema for `skit.toml` to stdout (useful for editor autocomplete):

```bash
skit config schema > skit-schema.json
```

## Repo Shortcuts

From the repo root:

```bash
just skit serve
just skit '--config skit.toml serve'
```

## Client CLI (`skit-cli`)

The optional client binary is `skit-cli` (crate: `streamkit-client`). It wraps common HTTP operations:

- `skit-cli oneshot <pipeline.yml> <input> <output> [--server URL]`
- `skit-cli create <pipeline.yml> [--name NAME] [--server URL]`
- `skit-cli destroy <session-id-or-name> [--server URL]`
- `skit-cli tune <session-id-or-name> <node-id> <param> <value-yaml> [--server URL]`
- `skit-cli list [--server URL]`
- `skit-cli shell [--server URL]`
- `skit-cli loadtest|lt [--config loadtest.toml] [...]`
- `skit-cli config [--server URL]`
- `skit-cli permissions [--server URL]`
- `skit-cli schema nodes|packets [--server URL]`
- `skit-cli pipeline <session-id-or-name> [--server URL]`
- `skit-cli plugins list|upload|delete [...] [--server URL]`
- `skit-cli samples list-oneshot|list-dynamic|get|save|delete [...] [--server URL]`
- `skit-cli assets list|upload|delete [...] [--server URL]`
- `skit-cli watch <session-id-or-name> [--pretty] [--server URL]`
- `skit-cli control nodes|pipeline|add-node|remove-node|connect|disconnect|validate-batch|apply-batch|tune-async [...] [--server URL]`

From the repo root, run it via `just`:

```bash
just skit-cli -- --help
```
