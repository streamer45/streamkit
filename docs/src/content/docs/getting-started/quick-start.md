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
TAG=v0.5.0 # replace with the latest release tag
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
TAG=v0.5.0 # replace with the latest release tag
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

# Build the embedded web UI first (requires Bun; creates ui/dist/)
just build-ui

just build-skit
just skit serve
```

## Verify

The Docker image binds to `0.0.0.0` inside the container, so built-in auth is enabled by default. Get the admin token first:

```bash
docker exec streamkit skit auth print-admin-token --raw
```

Open [http://localhost:4545](http://localhost:4545) in your browser and paste the token at the login screen.

> [!TIP]
> If you’re on **Linux** and want a frictionless demo (no login), you can use host networking with a loopback bind. In `auth.mode = "auto"`, this keeps built-in auth **disabled**:
>
> ```bash
> TAG=v0.5.0 # replace with the latest release tag
> docker run --rm -d --name streamkit \
>   --network host \
>   -e SK_SERVER__ADDRESS=127.0.0.1:4545 \
>   ghcr.io/streamer45/streamkit:${TAG}
> ```
>
> This only works on Linux. Docker Desktop for macOS/Windows does not support `--network host`.

> [!CAUTION]
> If you expose the server beyond localhost, keep auth enabled and follow the [Security](/guides/security/) guide.

## Run Your First Pipeline

Use a small but useful oneshot pipeline (audio gain), and get audio back:

If you started via Docker, copy the bundled sample pipeline and audio file out of the container:

```bash
docker cp streamkit:/opt/streamkit/samples/pipelines/oneshot/double_volume.yml ./double_volume.yml
docker cp streamkit:/opt/streamkit/samples/audio/system/sample.ogg ./sample.ogg
```

If you built from source, you already have both in the repo:

```bash
cp samples/pipelines/oneshot/double_volume.yml ./double_volume.yml
cp samples/audio/system/sample.ogg ./sample.ogg
```

Run the oneshot pipeline:

**Docker** (auth is enabled by default):

```bash
TOKEN=$(docker exec streamkit skit auth print-admin-token --raw)
curl -X POST http://localhost:4545/api/v1/process \
  -H "Authorization: Bearer $TOKEN" \
  -F config=@double_volume.yml \
  -F media=@sample.ogg \
  --output out.ogg
```

**Source build on localhost** (auth is disabled by default on loopback):

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
- [GPU Setup](/deployment/gpu/) - GPU-accelerated compositing and ML plugins
