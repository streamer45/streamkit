---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Quick Start
description: Get StreamKit running in 5 minutes
---

This guide gets you from zero to a working StreamKit installation in minutes.

## Prerequisites

- **Docker** (recommended), or **Rust** + **just** (build from source)

> [!NOTE]
> Official Docker images are published for `linux/amd64` (x86_64). On ARM hosts (Raspberry Pi, Apple Silicon, etc.), use “Build from Source” or run with amd64 emulation.

## Installation

### Option 1: Docker (recommended)

```bash
TAG=v0.1.0 # replace with the latest release tag
docker run --rm -d --name streamkit \
  -p 127.0.0.1:4545:4545/tcp \
  -p 127.0.0.1:4545:4545/udp \
  ghcr.io/streamer45/streamkit:${TAG} \
  skit serve # optional: this is the image default
```

To watch logs:

```bash
docker logs -f streamkit
```

To stop the container:

```bash
docker stop streamkit
```

### Option 2: GitHub Release + systemd (Linux)

```bash
TAG=v0.1.0 # replace with the latest release tag
curl -fsSL https://raw.githubusercontent.com/streamer45/streamkit/${TAG}/deploy/systemd/install.sh -o streamkit-install.sh
chmod +x streamkit-install.sh

sudo ./streamkit-install.sh --tag ${TAG}
```

> [!TIP]
> For convenience (less reproducible), you can install the latest release:
>
> ```bash
> curl -fsSL https://raw.githubusercontent.com/streamer45/streamkit/main/deploy/systemd/install.sh -o streamkit-install.sh
> chmod +x streamkit-install.sh
> sudo ./streamkit-install.sh --latest
> ```

### Option 3: Build from Source

```bash
git clone https://github.com/streamer45/streamkit.git
cd streamkit

# Build the embedded web UI (requires Bun)
just build-ui

just build-skit
just skit serve
```

## Verify

Open [http://localhost:4545](http://localhost:4545) in your browser. You should see the StreamKit dashboard.

> [!CAUTION]
> StreamKit does not currently implement authentication. If you expose the server beyond localhost, put it behind an authenticating reverse proxy and configure roles via a trusted header.

## Run Your First Pipeline

Use a small but useful oneshot pipeline (audio gain), and get audio back:

```bash
cat > double_volume.yml <<'YAML'
name: Volume Boost (2×)
mode: oneshot
steps:
  - kind: streamkit::http_input
  - kind: containers::ogg::demuxer
  - kind: audio::opus::decoder
  - kind: audio::gain
    params:
      gain: 2.0
  - kind: audio::opus::encoder
  - kind: containers::ogg::muxer
    params:
      channels: 2
      chunk_size: 65536
  - kind: streamkit::http_output
YAML
```

If you started via Docker, copy a bundled sample audio file out of the container:

```bash
docker cp streamkit:/opt/streamkit/samples/audio/system/sample.ogg ./sample.ogg
```

If you built from source instead, you already have the sample in the repo:

```bash
cp samples/audio/system/sample.ogg ./sample.ogg
```

Run the oneshot pipeline:

```bash
curl -X POST http://localhost:4545/api/v1/process \
  -F config=@double_volume.yml \
  -F media=@sample.ogg \
  --output out.ogg
```

> [!TIP]
> You can also run oneshot pipelines in the UI via the [Convert view](http://localhost:4545/convert).

## Next Steps

- [Installation Guide](/getting-started/installation/) - Detailed setup options
- [Creating Pipelines](/guides/creating-pipelines/) - Pipeline syntax and patterns
- [Web UI Guide](/guides/web-ui/) - Using the visual editor
