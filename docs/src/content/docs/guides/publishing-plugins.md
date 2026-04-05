---
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
# SPDX-License-Identifier: MPL-2.0
title: Publishing to a Registry
description: Package, sign, and publish plugins for the StreamKit marketplace
---

This guide covers the v1 registry format for publishing StreamKit plugins (native or WASM).

## Bundle layout

Each release ships a bundle archive (for example `bundle.tar.zst`) that contains the plugin binary
and optional license files:

```
libmy_plugin.so
plugin.yml
LICENSES/
```

The `plugin.yml` manifest is written into the bundle directory automatically during marketplace
installation so that the server can rediscover plugin metadata (including asset type declarations)
on restart. See [Plugin Bundle Convention](/architecture/plugin-bundle-convention/) for the
long-term directory layout proposal.

The entrypoint path in the manifest must match the plugin binary inside the bundle. The manifest and
signature are hosted alongside the bundle in the registry.

## Manifest format

`manifest.json` describes the plugin and bundle. Example:

```json
{
  "schema_version": 1,
  "id": "whisper",
  "name": "Whisper",
  "version": "1.2.3",
  "node_kind": "whisper",
  "kind": "native",
  "entrypoint": "libwhisper.so",
  "description": "Streaming speech-to-text using whisper.cpp",
  "license": "MIT",
  "bundle": {
    "url": "https://github.com/org/repo/releases/download/v1.2.3/bundle.tar.zst",
    "sha256": "abc123..."
  },
  "models": [
    {
      "id": "whisper-tiny-en-q5_1",
      "name": "Whisper tiny.en (q5_1)",
      "default": true,
      "source": "huggingface",
      "repo_id": "streamkit/whisper-models",
      "revision": "main",
      "files": ["ggml-tiny.en-q5_1.bin"],
      "sha256": "abc123..."
    }
  ]
}
```

`models[]` is optional. Files are downloaded into `[plugins].models_dir` and keep their relative
paths. When `id`/`name`/`default` are provided, the UI allows selecting which models to download.
If a model file is an archive (`.tar`, `.tar.gz`, `.tar.bz2`, `.tar.zst`, etc.), StreamKit
extracts it into `models_dir` after download.

For the official registry, use `scripts/marketplace/upload_models_to_hf.py` to upload mirrored
model files to a per-plugin Hugging Face repo (for example, `streamkit/whisper-models`) before
publishing manifests.

When manifests reference `.tar.*` bundles, pass `--create-archives` to build the archives from the
local model directories before upload.

## Sign the manifest

StreamKit uses minisign-compatible Ed25519 signatures.

```bash
# Create a keypair (once)
minisign -G -p streamkit.pub -s streamkit.key

# Sign the manifest
minisign -S -s streamkit.key -m manifest.json -x manifest.minisig
```

Server admins must add the public key to `[plugins].trusted_pubkeys`.

## Registry index

Registries are static JSON files served over HTTPS. Example `index.json`:

```json
{
  "schema_version": 1,
  "plugins": [
    {
      "id": "whisper",
      "name": "Whisper",
      "description": "Streaming speech-to-text using whisper.cpp",
      "latest": "1.2.3",
      "versions": [
        {
          "version": "1.2.3",
          "manifest_url": "https://example.com/plugins/whisper/1.2.3/manifest.json",
          "signature_url": "https://example.com/plugins/whisper/1.2.3/manifest.minisig",
          "published_at": "2025-01-24"
        }
      ]
    }
  ]
}
```

By default, StreamKit requires HTTPS and blocks non-public hosts. Same-origin enforcement is
optional; when enabled, manifest/signature/bundle URLs must share origin with the registry index.
If you host bundles or manifests on a different origin, admins must allowlist that origin (and
explicitly allow HTTP if they choose to use it).

## Recommended hosting

- Registry metadata: GitHub Pages (static `index.json` + per-version manifests).
- Bundles: GitHub Releases (large immutable binaries).

If you mix GitHub Pages (registry/manifest) with GitHub Releases (bundles) and admins enable
`marketplace_require_registry_origin`, they must allowlist the release hosts because redirects are
validated per hop. The allowlist should include `https://github.com` plus the release asset hosts
(commonly `https://objects.githubusercontent.com` and `https://release-assets.githubusercontent.com`).

See [Installing Plugins](/guides/installing-plugins/) for the admin workflow.
