---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Model Downloads
description: Server-side model downloads for marketplace plugins
---

Marketplace plugins can declare model assets in their `manifest.json`. StreamKit downloads models
server-side and never exposes tokens to the browser.

## Configuration

```toml
[plugins]
models_dir = "/var/lib/streamkit/models" # defaults to ./models
huggingface_token = "${HF_TOKEN}"        # required for gated Hugging Face models
allow_model_urls = false                 # set true to allow ModelSource::Url
```

`models_dir` is where model files are written. The paths inside `models[]` are preserved under this
directory.

## Manifest fields

Example `models[]` entries:

```json
[
  {
    "id": "whisper-tiny-en-q5_1",
    "name": "Whisper tiny.en (q5_1)",
    "default": true,
    "source": "huggingface",
    "repo_id": "streamkit/whisper-models",
    "revision": "main",
    "files": ["ggml-tiny.en-q5_1.bin"],
    "sha256": "..."
  },
  {
    "id": "silero-vad",
    "name": "Silero VAD",
    "default": true,
    "source": "url",
    "url": "https://example.com/models/ten-vad.onnx",
    "sha256": "abc123..."
  }
]
```

Official plugins mirror models under `streamkit/<plugin>-models` (for example, `streamkit/whisper-models`).

If a model is `gated: true`, StreamKit requires a Hugging Face token to download it.

`source: "url"` entries are disabled by default. When enabled, model URLs go through the same
marketplace URL policy (HTTPS required by default, blocked host/IP ranges, and optional same-origin
enforcement).

Model entries may include `id`, `name`, and `default`. When present, the UI lets admins select
which models to download and preselects those marked `default`.

Model files can be archives (`.tar`, `.tar.gz`, `.tgz`, `.tar.bz2`, `.tbz2`, `.tar.zst`, `.tzst`).
When an archive is downloaded, StreamKit extracts it into `models_dir` and keeps the archive file.

## UI behavior

The Marketplace install panel includes a **Download models after install** toggle when models are
declared. When model IDs are present, the UI shows a checklist for selective downloads. Progress
is tracked as part of the install job.

## License disclosure

Plugin authors should include `license` or `license_url` for each model. The UI prompts admins to
acknowledge model licenses before installing.
