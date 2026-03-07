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

## Running E2E tests

End-to-end tests live in `e2e/` and use Playwright (Chromium, headless).

1. **Build the UI** and **start the server** in one terminal:

   ```bash
   just build-ui && SK_SERVER__MOQ_GATEWAY_URL=http://127.0.0.1:4545/moq SK_SERVER__ADDRESS=127.0.0.1:4545 just skit
   ```

2. **Run the tests** in a second terminal:

   ```bash
   just e2e-external http://localhost:4545
   ```

### Headless-browser pitfalls

- Playwright runs headless Chromium with a default 1280×720 viewport.
  Elements rendered below the fold are **not visible** to
  `IntersectionObserver`. If a test relies on an element being observed
  (e.g. the `<canvas>` used by the MoQ video renderer), scroll it into
  view first:

  ```ts
  const canvas = page.locator('canvas');
  await canvas.scrollIntoViewIfNeeded();
  ```

- The `@moq/watch` `Video.Renderer` enables the `Video.Decoder` (and
  therefore the `video/data` MoQ subscription) **only** when the canvas is
  intersecting. Forgetting to scroll will result in a permanently black
  canvas.

## Render performance profiling

StreamKit ships a two-layer profiling infrastructure for detecting render
regressions — particularly **cascade re-renders** where a slider interaction
(opacity, rotation) triggers expensive re-renders in unrelated memoized
components (`UnifiedLayerList`, `OpacityControl`, `RotationControl`, etc.).

### When to use this

- **After touching compositor hooks or components** (`useCompositorLayers`,
  `CompositorNode`, or any `React.memo`'d sub-component): run the perf tests
  to verify you haven't broken memoization barriers.
- **When optimising render performance**: use the baseline comparison to
  measure before/after render counts and durations.
- **In CI**: Layer 1 tests run automatically via `just perf-ui` and will fail
  if render counts regress beyond the 2σ threshold stored in the baseline.

### Layer 1 — Component-level regression tests (Vitest)

Fast, deterministic tests that measure hook/component render counts in
happy-dom. No browser required.

```bash
just perf-ui          # runs all *.perf.test.* files
```

Key files:

| File | Purpose |
|------|---------|
| `ui/src/test/perf/measure.ts` | `measureRenders()` (components) and `measureHookRenders()` (hooks) |
| `ui/src/test/perf/compare.ts` | Baseline read/write, 2σ comparison, report formatting |
| `ui/src/hooks/useCompositorLayers.render-perf.test.ts` | Cascade re-render regression tests |
| `perf-baselines.json` (repo root) | Baseline snapshot — committed to track regressions over time |

**Cascade detection pattern**: the render-perf tests simulate rapid slider
drags (20 ticks of opacity/rotation) and assert that total render count stays
within a budget (currently ≤ 30). If callback references become unstable
(e.g. `layers` array in deps instead of `selectedLayerKind`), React.memo
barriers break and the render count will blow past the budget, failing the
test.

### Layer 2 — Interaction-level profiling (Playwright + React.Profiler)

Real-browser profiling for dev builds. Components wrapped with
`React.Profiler` push metrics to `window.__PERF_DATA__` which Playwright
tests can read via `page.evaluate()`.

```bash
just perf-e2e         # requires: just skit + just ui (dev server at :3045)
```

Key files:

| File | Purpose |
|------|---------|
| `ui/src/perf/profiler.ts` | Dev-only `PerfProfiler` wrapper + `window.__PERF_DATA__` store |
| `e2e/tests/perf-helpers.ts` | `capturePerfData()` / `resetPerfData()` Playwright utilities |
| `e2e/tests/compositor-perf.spec.ts` | E2E test: creates PiP session, drags all sliders, asserts render budget |

Use Layer 2 when you need real paint/layout timing or want to profile
interactions end-to-end with actual browser rendering.

### Updating the baseline

Run `just perf-ui` — the last test in the render-perf suite writes a fresh
`perf-baselines.json` (gated behind `UPDATE_PERF_BASELINE=1`, which the
`test:perf` script sets automatically). Regular `just test-ui` runs compare
against the baseline but never overwrite it. Commit the updated baseline
alongside your changes so future runs compare against the new numbers.

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
