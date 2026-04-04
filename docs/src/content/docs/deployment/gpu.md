---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: GPU Setup
description: Configure GPU acceleration for compositing and ML workloads
---

StreamKit can use GPUs in two ways: the built-in video compositor uses **wgpu** (Vulkan/Metal/DX12 — works with any compatible GPU), and selected native ML plugins use **NVIDIA CUDA** for inference acceleration.

## Compositor (GPU compositing)

The built-in `video::compositor` node supports GPU-accelerated compositing via wgpu. Set `gpu_mode` in the compositor params:

| Value  | Behaviour |
|--------|-----------|
| `auto` (default) | CPU for simple scenes, auto-promotes to GPU when complexity warrants it |
| `gpu`  | Force GPU compositing (falls back to CPU if no GPU is available) |
| `cpu`  | Force CPU-only compositing (tiny-skia) |

No NVIDIA-specific tooling is required for compositor GPU — wgpu works with any Vulkan-capable GPU. Example:

```yaml
compositor:
  kind: video::compositor
  params:
    width: 1280
    height: 720
    gpu_mode: auto  # or "gpu" / "cpu"
```

## ML Plugins (NVIDIA GPU)

StreamKit can use NVIDIA GPUs for selected native ML plugins. GPU support depends on how you build and deploy:

- `ghcr.io/streamer45/streamkit:latest` (CPU) runs plugins on CPU.
- `ghcr.io/streamer45/streamkit:latest-gpu` is a convenience tag for GPU deployments. The server image itself is the same; GPU acceleration comes from GPU-capable plugins and running the container with `--gpus all`.

## Requirements (host)

- NVIDIA driver installed (container uses the host driver)
- `nvidia-container-toolkit` configured for Docker

## Quick Start (GPU image)

```bash
TAG=v0.2.0 # replace with the latest release tag
docker run --rm \
  --gpus all \
  -p 127.0.0.1:4545:4545/tcp \
  -p 127.0.0.1:4545:4545/udp \
  ghcr.io/streamer45/streamkit:${TAG}-gpu \
  skit serve # optional: this is the image default
```

Official images do not bundle ML models or plugins; mount them as needed (see [Docker Deployment](/deployment/docker/)).

## Verify GPU access

```bash
nvidia-smi
docker run --rm --gpus all nvidia/cuda:12.3.1-base-ubuntu22.04 nvidia-smi
```

StreamKit doesn't expose a dedicated "GPU status" endpoint. Confirm GPU execution by enabling GPU parameters in a pipeline and checking logs.

## Enable GPU in pipelines

### Whisper (STT)

Start from `samples/pipelines/oneshot/speech_to_text.yml` and add:

```yaml
- kind: plugin::native::whisper
  params:
    use_gpu: true
    gpu_device: 0
```

### Kokoro (TTS)

Start from `samples/pipelines/oneshot/kokoro-tts.yml` and add:

```yaml
- kind: plugin::native::kokoro
  params:
    execution_provider: cuda   # or "tensorrt" if available in your build
```

## Multi-GPU

Set `gpu_device` per pipeline/plugin instance (0-based).

## Next Steps

- [Docker Deployment](/deployment/docker/) - Container setup and volumes
- [Configuration](/reference/configuration/) - Plugins, resources, and limits
