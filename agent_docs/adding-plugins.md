<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Adding an Official Plugin

When making a plugin official and downloadable from the registry, update all of
the following:

- Plugin source under `plugins/native/<id>/` (crate metadata + README).
- Plugin metadata in `plugins/native/<id>/plugin.yml` (id, version, entrypoint,
  artifact path, models, licenses, homepage/repo).
- Generate `marketplace/official-plugins.json` with
  `scripts/marketplace/generate_official_plugins.py` and commit the result.
- Build list in `scripts/marketplace/build_official_plugins.sh`.
- Build prerequisites in `.github/workflows/release.yml` if new system deps are
  required to compile or package the plugin.
- Bundle/registry smoke check: run `scripts/marketplace/build_registry.py` and
  `scripts/marketplace/verify_bundles.py` locally.
- Portability review: run `scripts/marketplace/verify_bundles.py` which checks
  NEEDED deps, RUNPATH/RPATH, and reports portability issues.
- Docs: add/update the plugin page under
  `docs/src/content/docs/reference/plugins/` and list it in
  `docs/src/content/docs/reference/plugins/index.md` if applicable.
- Runtime shared libs: if the plugin needs bundled `.so` files, ensure the
  bundle includes them and the entrypoint RUNPATH uses `$ORIGIN`, and update the
  portability gate in `scripts/marketplace/verify_bundles.py` as needed.
- **Models**: if the plugin relies on ML models, upload them to the StreamKit
  Hugging Face repo so they remain accessible indefinitely (license permitting).
- **Human review required** before bundling any new third-party shared libraries
  (licensing, security, size, and distro compatibility).

## GPU (CUDA) bundle variants

A plugin can ship an optional `cuda` bundle variant alongside its canonical CPU
bundle. Clients auto-detect CUDA at install time (or honour an explicit
`accelerator`) and fall back to the CPU bundle when no GPU is present. To make a
plugin CUDA-capable:

- Declare `accelerators: [cpu, cuda]` in `plugins/native/<id>/plugin.yml` (the
  default is CPU-only). This is propagated into `marketplace/official-plugins.json`.
- For compile-time GPU plugins (e.g. whisper, helsinki), add a `cuda` Cargo
  feature that enables the backend's CUDA path. `build_official_plugins_cuda.sh`
  builds with `--features cuda` automatically when the feature exists.
- sherpa-onnx plugins (kokoro, sensevoice, vad, matcha) are execution-provider
  agnostic: the same `.so` is repackaged against the CUDA sherpa runtime, so no
  feature flag is needed — just the `accelerators` declaration.
- The CUDA registry pass runs on the self-hosted Ada GPU runner in
  `.github/workflows/marketplace-build.yml` (`build-marketplace-cuda` job). It
  vendors the GPU ONNX Runtime provider libs (`libonnxruntime_providers_cuda.so`,
  `libonnxruntime_providers_shared.so`) and layers a `cuda` variant onto the
  already-published CPU manifest (append-only; a published variant is immutable).
- CUDA bundles are named `<id>-<ver>-cuda-bundle.tar.zst` and uploaded to the
  same per-plugin release as the CPU bundle. `verify_bundles.py --accelerator
  cuda` permits CUDA NEEDED/RUNPATH deps (libcudart/libcublas/libcudnn) that the
  strict CPU gate rejects.
