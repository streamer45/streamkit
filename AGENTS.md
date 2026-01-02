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

## Workflow expectations

- Keep PRs focused and minimal.
- Run `just test` and `just lint` when making code changes (or explain why you couldn't).
- Follow `CONTRIBUTING.md` (DCO sign-off, Conventional Commits, SPDX headers where applicable).
- **Linting discipline**: Do not blindly suppress lint warnings or errors with ignore/exception rules. Instead, consider refactoring or improving the code to address the underlying issue. If an exception is truly necessary, it **must** include a comment explaining the rationale.

## Docker notes

- Official images are built from `Dockerfile` (CPU) and `Dockerfile.gpu` (GPU-tagged) via `.github/workflows/docker.yml`.
- `/healthz` is the lightweight health endpoint (also `/health`).
- Official images do not bundle ML models or plugins; they are expected to be mounted at runtime.
