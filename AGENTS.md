<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# StreamKit agent notes

These notes apply to coding agents (Claude/Codex/etc.) contributing to this repo.

## Supervision requirement

Agent-assisted contributions are welcome, but should be **supervised** and **reviewed by a human** before merge. Treat agent output as untrusted: verify correctness, security, licensing, and style.

## Project basics

- **Supported platform**: Linux x86_64 (for now).
- **Primary server binary**: `skit` (crate: `streamkit-server`).
- **Dev task runner**: `just` (see `justfile`).
- **Docs**: Astro + Starlight in `docs/` (sidebar in `docs/astro.config.mjs`).
- **UI tooling**: Bun-first. Use `bun install`, `bunx` (or `bun run` scripts) for UI work—avoid npm/pnpm.

## Workflow expectations

- Keep PRs focused and minimal.
- Run `just test` and `just lint` when making code changes (or explain why you couldn't).
- Follow `CONTRIBUTING.md` (DCO sign-off, Conventional Commits, SPDX headers where applicable).
- **Linting discipline**: Do not blindly suppress lint warnings or errors with ignore/exception rules. Instead, consider refactoring or improving the code to address the underlying issue. If an exception is truly necessary, it **must** include a comment explaining the rationale.

## Docker notes

- Official images are built from `Dockerfile` (CPU) and `Dockerfile.gpu` (GPU-tagged) via `.github/workflows/docker.yml`.
- `/healthz` is the lightweight health endpoint (also `/health`).
- Official images do not bundle ML models or plugins; they are expected to be mounted at runtime.

## Adding an official plugin

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
- Portability table in `marketplace/PORTABILITY_REVIEW.md` (NEEDED deps,
  RUNPATH/RPATH, recommendation).
- Docs: add/update the plugin page under
  `docs/src/content/docs/reference/plugins/` and list it in
  `docs/src/content/docs/reference/plugins/index.md` if applicable.
- Runtime shared libs: if the plugin needs bundled `.so` files, ensure the
  bundle includes them and the entrypoint RUNPATH uses `$ORIGIN`, and update the
  portability gate in `scripts/marketplace/verify_bundles.py` as needed.
- **Human review required** before bundling any new third-party shared libraries
  (licensing, security, size, and distro compatibility).
