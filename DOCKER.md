<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# StreamKit Docker Guide

> [!NOTE]
> The documentation site is the canonical source for deployment guidance: <https://streamkit.dev/deployment/docker/>.
> This file is a repo-local snapshot for convenience and may lag behind the website docs.

This guide covers building and running StreamKit Docker images. The Docker images are slim (~200-400+ MB) and contain the server binary (with the web UI embedded), sample pipelines, and a few small audio samples. Models and plugins must be mounted externally.

> [!NOTE]
> Official Docker images are published for `linux/amd64` (x86_64). On ARM hosts (Raspberry Pi, Apple Silicon, etc.), use “Build from Source” or run with amd64 emulation.

## Quick Start

### 1. Build the Image

```bash
# Build CPU image
docker build -t streamkit:latest -f Dockerfile .

# Build time: 5-10 minutes (first build), 2-3 minutes (cached)
```

### 2. Download Models and Build Plugins

Models and plugins are not included in the Docker image. Download them separately:

```bash
# Download all models (~2GB total, one-time)
just download-models

# Build all native plugins (requires sherpa-onnx installed)
just setup-kokoro    # Install sherpa-onnx + download Kokoro models
just build-plugins   # Build all plugins
just copy-plugins    # Copy to .plugins/ directory
```

### 3. Run with Mounted Models and Plugins

```bash
docker run --rm -d --name streamkit \
  -p 127.0.0.1:4545:4545/tcp \
  -p 127.0.0.1:4545:4545/udp \
  -v $(pwd)/models:/opt/streamkit/models:ro \
  -v $(pwd)/.plugins:/opt/streamkit/plugins:ro \
  streamkit:latest

# Note: the image defaults to `skit serve` (you can also pass it explicitly).

# Open http://localhost:4545 in your browser
# To stop: docker stop streamkit
```

## Image Details

### Slim Image

- **Size**: ~200-400 MB
- **Base**: Debian Bookworm Slim
- **Contents**: Server binary (UI embedded) + sample pipelines + small audio samples (`.ogg`/`.opus`)
- **Models/Plugins**: Must be mounted externally

The slim approach provides:
- Fast builds (~5-10 min vs 30-45 min)
- Small images (~200 MB vs 2-4 GB)
- Clean licensing (no bundled third-party models)
- Flexibility to choose exactly which models to use

## Setting Up Models and Plugins

### Download Models

Individual model download commands:

```bash
# Whisper STT models
just download-whisper-models    # Downloads ggml-base.en-q5_1.bin
just download-silero-vad        # Downloads silero_vad.onnx for Whisper VAD

# TTS models
just download-kokoro-models     # Kokoro v1.1 (103 voices, 24kHz)
just download-piper-models      # Piper English
just download-matcha-models     # Matcha TTS

# Other models
just download-sensevoice-models # SenseVoice multilingual STT
just download-tenvad-models     # ten-vad for VAD plugin

# Translation (WARNING: CC-BY-NC-4.0 license - non-commercial only)
just download-nllb-models       # NLLB-200 (200 languages)

# Download all models at once
just download-models
```

### Build Plugins

Native plugins require dependencies to be installed first:

```bash
# Install sherpa-onnx (required for Kokoro, Piper, SenseVoice, VAD, Matcha)
just install-sherpa-onnx

# Build individual plugins
just build-plugin-native-whisper
just build-plugin-native-kokoro
just build-plugin-native-piper
just build-plugin-native-sensevoice
just build-plugin-native-vad
just build-plugin-native-matcha
just build-plugin-native-nllb

# Build all plugins
just build-plugins-native

# Copy to .plugins/ directory
just copy-plugins
```

## Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `RUST_LOG` | Rust log filter | `info` |
| `SK_SERVER__ADDRESS` | Bind address (`host:port`) | `0.0.0.0:4545` (in official images) |
| `SK_PLUGINS__DIRECTORY` | Plugin base dir | `/opt/streamkit/plugins` (in official images) |
| `SK_PLUGINS__ALLOW_HTTP_MANAGEMENT` | Allow plugin upload/delete via HTTP APIs | `false` |
| `SK_SERVER__SAMPLES_DIR` | Sample pipelines dir | `/opt/streamkit/samples/pipelines` (in official images) |
| `SK_SERVER__MOQ_GATEWAY_URL` | MoQ/WebTransport gateway URL for the frontend | `http://127.0.0.1:4545/moq` (in official images) |
| `OPENAI_API_KEY` | OpenAI API key for voice agent pipelines | - |

## Volume Mounts

### Required Mounts

For full functionality, mount models and plugins:

```bash
docker run --rm \
  -p 127.0.0.1:4545:4545/tcp \
  -p 127.0.0.1:4545:4545/udp \
  -v $(pwd)/models:/opt/streamkit/models:ro \
  -v $(pwd)/.plugins:/opt/streamkit/plugins:ro \
  streamkit:latest
```

### Optional Mounts

```bash
# Custom configuration
-v $(pwd)/skit.toml:/opt/streamkit/skit.toml:ro

# Persistent logs
-v streamkit-logs:/opt/streamkit/logs

# Additional plugins directory
-v $(pwd)/custom-plugins:/opt/streamkit/.plugins:ro
```

## Docker Compose Example

Create `docker-compose.yml`:

```yaml
services:
  streamkit:
    build:
      context: .
      dockerfile: Dockerfile
    container_name: streamkit
    restart: unless-stopped
    ports:
      - "127.0.0.1:4545:4545/tcp"
      - "127.0.0.1:4545:4545/udp"
    environment:
      - RUST_LOG=info
      - OPENAI_API_KEY=${OPENAI_API_KEY:-}
    volumes:
      - ./models:/opt/streamkit/models:ro
      - ./.plugins:/opt/streamkit/plugins:ro
      - streamkit-logs:/opt/streamkit/logs

volumes:
  streamkit-logs:
```

Run with:

```bash
docker-compose up
```

## Networking

### MoQ / WebTransport (QUIC)

MoQ uses WebTransport over QUIC (UDP). For current StreamKit builds, MoQ/WebTransport listens on the **same port as the HTTP server**.

Publish both TCP and UDP for the same port (default: `4545`):

```bash
docker run --rm \
  -p 127.0.0.1:4545:4545/tcp \
  -p 127.0.0.1:4545:4545/udp \
  -v $(pwd)/models:/opt/streamkit/models:ro \
  -v $(pwd)/.plugins:/opt/streamkit/plugins:ro \
  streamkit:latest
```

### Behind a Reverse Proxy

Example with Caddy:

```
streamkit.example.com {
    reverse_proxy streamkit:4545
}
```

Example with Nginx:

```nginx
server {
    listen 443 ssl;
    server_name streamkit.example.com;

    location / {
        proxy_pass http://streamkit:4545;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
    }
}
```

## Health Checks

Images include built-in health checks that query the `/healthz` endpoint:

```bash
# Check container health
docker inspect --format='{{.State.Health.Status}}' streamkit

# View health check logs
docker inspect --format='{{range .State.Health.Log}}{{.Output}}{{end}}' streamkit
```

Health check configuration:
- **Interval**: 30s
- **Timeout**: 3s
- **Start period**: 5s
- **Retries**: 3

## Building Images Locally

### CPU Image

```bash
# Build
docker build -t streamkit:latest -f Dockerfile .

# Run
docker run --rm -d --name streamkit \
  -p 127.0.0.1:4545:4545/tcp \
  -p 127.0.0.1:4545:4545/udp \
  -v $(pwd)/models:/opt/streamkit/models:ro \
  -v $(pwd)/.plugins:/opt/streamkit/plugins:ro \
  streamkit:latest
```

Build time: 5-10 minutes (first build), 2-3 minutes (cached)

### GPU Image

The GPU Dockerfile is identical to the CPU version since plugins are mounted externally. For GPU support:

1. Build GPU-enabled plugins separately (with CUDA support)
2. Mount the GPU plugins at runtime
3. Run with `--gpus all`

```bash
# Build image (same as CPU)
docker build -t streamkit:gpu -f Dockerfile.gpu .

# Run with GPU and GPU-compiled plugins
docker run --rm --gpus all \
  -p 127.0.0.1:4545:4545/tcp \
  -p 127.0.0.1:4545:4545/udp \
  -v $(pwd)/models:/opt/streamkit/models:ro \
  -v $(pwd)/.plugins-gpu:/opt/streamkit/plugins:ro \
  streamkit:gpu
```

## Troubleshooting

### Container Won't Start

```bash
# Check logs
docker logs streamkit

# Check resource limits
docker stats streamkit

# Verify port availability
lsof -i :4545
```

### Plugins Not Loading

```bash
# Verify plugin files exist in mounted directory
docker exec streamkit ls -la /opt/streamkit/plugins/native/

# Check server logs for plugin loading errors
docker logs streamkit 2>&1 | grep -i plugin

# Verify sherpa-onnx libraries are available (for TTS/STT plugins)
# Note: GPU plugins may require additional CUDA libraries
```

### Health Check Failing

```bash
# Check if server is listening
docker exec streamkit curl -v http://localhost:4545/healthz

# Check logs for errors
docker logs streamkit 2>&1 | grep -i error
```

### Out of Memory

Increase Docker memory limits:

```bash
# Docker Desktop: Settings > Resources > Memory
# Docker CLI:
docker run --rm \
  --memory="4g" \
  --memory-swap="8g" \
  -p 127.0.0.1:4545:4545/tcp \
  -p 127.0.0.1:4545:4545/udp \
  -v $(pwd)/models:/opt/streamkit/models:ro \
  -v $(pwd)/.plugins:/opt/streamkit/plugins:ro \
  streamkit:latest
```

## Support

- **Documentation**: [Main README](README.md)
- **Issues**: [GitHub Issues](https://github.com/streamer45/streamkit/issues)
- **License**: MPL-2.0

## Image Metadata

View image metadata:

```bash
docker inspect streamkit:latest | jq '.[0].Config.Labels'
```

Check image size:

```bash
docker images streamkit
```
