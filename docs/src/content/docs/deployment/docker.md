---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Docker Deployment
description: Deploy StreamKit with Docker and Docker Compose
---

StreamKit provides Docker images for easy deployment.

## Quick Start

Official images are published publicly to GitHub Container Registry (GHCR). Pulls do not require `docker login`.

```bash
TAG=v0.2.0 # replace with the latest release tag
docker run --rm --name streamkit \
  -p 127.0.0.1:4545:4545/tcp \
  -p 127.0.0.1:4545:4545/udp \
  ghcr.io/streamer45/streamkit:${TAG} \
  skit serve # optional: this is the image default
```

### Authenticate (built-in auth)

The official images bind to `0.0.0.0:4545` inside the container so published ports work. That means built-in auth is enabled by default (`auth.mode = "auto"`).

Print the bootstrap admin token and paste it into the Web UI at `/login`:

```bash
docker exec streamkit skit auth print-admin-token --raw
```

### Local demo without login (Linux-only)

If you want a frictionless demo (no login) and you’re running on **Linux**, you can run StreamKit with host networking and bind to loopback inside the container. In `auth.mode = "auto"`, this keeps built-in auth **disabled**:

```bash
TAG=v0.2.0 # replace with the latest release tag
docker run --rm -d --name streamkit \
  --network host \
  -e SK_SERVER__ADDRESS=127.0.0.1:4545 \
  ghcr.io/streamer45/streamkit:${TAG}
```

This mode is intended for local demos/dev only (it doesn’t work on Docker Desktop for macOS/Windows, and it changes the container isolation model).

## Demo Image (Batteries Included)

StreamKit also publishes a `-demo` image intended for demos/evaluation. It bundles core plugins plus the models needed by the shipped sample pipelines, so it should work out of the box (but is much larger than the slim images).

```bash
docker run --rm --name streamkit-demo \
  -p 127.0.0.1:4545:4545/tcp \
  -p 127.0.0.1:4545:4545/udp \
  ghcr.io/streamer45/streamkit:${TAG}-demo
```

> [!NOTE]
> In Docker, StreamKit binds to `0.0.0.0` inside the container so published ports work. With `auth.mode=auto`, built-in auth is enabled by default.
> To log in, print the bootstrap admin token and paste it into the Web UI at `/login`:
>
> ```bash
> docker exec streamkit-demo skit auth print-admin-token --raw
> ```

If you want the OpenAI-powered sample pipelines, pass `OPENAI_API_KEY` without putting it directly in the command:

```bash
# Inherit OPENAI_API_KEY from your current shell environment (recommended).
# (Make sure it's set on the host before you run this.)
docker run --rm --env OPENAI_API_KEY \
  -p 127.0.0.1:4545:4545/tcp -p 127.0.0.1:4545:4545/udp \
  ghcr.io/streamer45/streamkit:${TAG}-demo
```

Or use an env-file so the secret never appears in your shell history:

```bash
printf 'OPENAI_API_KEY=%s\n' 'sk-...' > streamkit.env
chmod 600 streamkit.env
docker run --rm --env-file streamkit.env \
  -p 127.0.0.1:4545:4545/tcp -p 127.0.0.1:4545:4545/udp \
  ghcr.io/streamer45/streamkit:${TAG}-demo
```

### Debugging native crashes (gdb)

The demo image includes `gdb`. To attach to the running server inside Docker, run with ptrace enabled:

```bash
docker run --rm --name streamkit-demo \
  --cap-add=SYS_PTRACE \
  --security-opt seccomp=unconfined \
  --user root \
  -p 127.0.0.1:4545:4545/tcp -p 127.0.0.1:4545:4545/udp \
  ghcr.io/streamer45/streamkit:${TAG}-demo

ps -eo pid,cmd
gdb -p 1
```

> [!NOTE]
> Official Docker images are published for `linux/amd64` (x86_64). On ARM hosts, use “Build from Source” or run with amd64 emulation.

> [!NOTE]
> The official images ship with `/opt/streamkit/skit.toml` (see `docker-skit.toml` (CPU) / `docker-skit-gpu.toml` (GPU) in the repo). It binds to `0.0.0.0:4545` inside the container so published ports work, but you should publish/bind those ports to localhost (recommended) or otherwise firewall them.

> [!CAUTION]
> StreamKit ships with built-in authentication. Do not disable auth when exposing it to untrusted networks. See [Authentication](/guides/authentication/) and [Security](/guides/security/).

## Docker Compose

### Basic Setup

```yaml
# docker-compose.yml
services:
  streamkit:
    image: ghcr.io/streamer45/streamkit:v0.2.0 # replace with the latest release tag
    ports:
      - "127.0.0.1:4545:4545/tcp"
      - "127.0.0.1:4545:4545/udp"
    command: ["skit", "serve"]
    restart: unless-stopped
    volumes:
      # Recommended: persist auth state (keys + token index) across restarts.
      - streamkit-state:/opt/streamkit/.streamkit
      # Optional: persist dynamically loaded plugins.
      - streamkit-plugins:/opt/streamkit/plugins

volumes:
  streamkit-state:
  streamkit-plugins:
```

### Demo Image with Secrets

Use `env_file` to avoid putting secrets in your `docker-compose.yml`:

```yaml
services:
  streamkit:
    image: ghcr.io/streamer45/streamkit:v0.2.0-demo
    env_file:
      - ./streamkit.env
```

## Building Images

### CPU-only Image

```bash
docker build -t streamkit:latest .
```

### GPU Image

```bash
docker build -f Dockerfile.gpu -t streamkit:gpu .
```

`Dockerfile.gpu` builds the same server image as the CPU Dockerfile. GPU acceleration comes from running with `--gpus all` and using GPU-capable plugins; see [GPU Setup](/deployment/gpu/).

## Configuration

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `SK_SERVER__ADDRESS` | `0.0.0.0:4545` | Bind address (`host:port`) (default in the official images) |
| `SK_PLUGINS__DIRECTORY` | `/opt/streamkit/plugins` | Plugin directory (default in the official images) |
| `SK_RESOURCES__KEEP_MODELS_LOADED` | `true` | Cache ML models |
| `SK_SERVER__MOQ_GATEWAY_URL` | `http://127.0.0.1:4545/moq` | (MoQ builds) URL the frontend uses for WebTransport (override for non-local deployments) |
| `SK_AUTH__STATE_DIR` | `/opt/streamkit/.streamkit/auth` | Auth state directory in official images; mount `/opt/streamkit/.streamkit` to persist it |

### Volume Mounts

| Path | Purpose |
|------|---------|
| `/opt/streamkit/models` | ML models (Whisper, Kokoro) |
| `/opt/streamkit/plugins` | Plugin directory (default in official Docker images) |
| `/opt/streamkit/.plugins` | Optional plugin directory if you set `SK_PLUGINS__DIRECTORY=/opt/streamkit/.plugins` |
| `/opt/streamkit/.streamkit` | Auth state (keys, token index, bootstrap admin token) |
| `/opt/streamkit/skit.toml` | Configuration file (default config shipped in the image) |

## Health Checks

```yaml
services:
  streamkit:
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:4545/healthz"]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 10s
```

## Production Considerations

### Resource Limits

```yaml
services:
  streamkit:
    deploy:
      resources:
        limits:
          cpus: '4'
          memory: 8G
        reservations:
          cpus: '2'
          memory: 4G
```

### Logging

```yaml
services:
  streamkit:
    logging:
      driver: json-file
      options:
        max-size: "10m"
        max-file: "3"
```

### Reverse Proxy (Caddy)

```yaml
services:
  caddy:
    image: caddy:2
    ports:
      - "80:80"
      - "443:443"
    volumes:
      - ./Caddyfile:/etc/caddy/Caddyfile
      - caddy-data:/data

  streamkit:
    image: ghcr.io/streamer45/streamkit:latest
    expose:
      - "4545"

volumes:
  caddy-data:
```

```
# Caddyfile
streamkit.example.com {
    reverse_proxy streamkit:4545
}
```

### MoQ / WebTransport (QUIC)

MoQ uses WebTransport over QUIC (UDP). Typical HTTP reverse proxies like Caddy or nginx will not handle this traffic natively.

StreamKit’s built-in MoQ WebTransport acceptor listens on the **same host:port as the HTTP server** (`[server].address`) and expects you to publish both TCP and UDP for that port (e.g. `4545/tcp` + `4545/udp`).

TLS notes:

- If you configure TLS via `[server].tls`, `[server].cert_path`, and `[server].key_path`, StreamKit uses those certificates for WebTransport.
- Otherwise, StreamKit auto-generates a short-lived self-signed certificate (intended for local development). Fingerprints are exposed via `GET /api/v1/moq/fingerprints` (also `GET /certificate.sha256` for the first fingerprint).
- For Docker-to-browser local testing, keep `SK_SERVER__MOQ_GATEWAY_URL=http://127.0.0.1:4545/moq` so the UI connects to the published host port, not the container-internal address.

If you deploy MoQ in production, plan for a QUIC/WebTransport-aware gateway or an L4 load balancer to route UDP/QUIC traffic, alongside your normal HTTP reverse proxy for the UI/API.

## Next Steps

- [GPU Setup](/deployment/gpu/) - Enable GPU acceleration
- [Performance Tuning](/guides/performance/) - Tune latency vs throughput
