<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Render Performance Profiling

StreamKit ships a two-layer profiling infrastructure for detecting render
regressions — particularly **cascade re-renders** where a slider interaction
(opacity, rotation) triggers expensive re-renders in unrelated memoized
components (`UnifiedLayerList`, `OpacityControl`, `RotationControl`, etc.).

## When to Use This

- **After touching compositor hooks or components** (`useCompositorLayers`,
  `CompositorNode`, or any `React.memo`'d sub-component): run the perf tests
  to verify you haven't broken memoization barriers.
- **When optimising render performance**: use the baseline comparison to
  measure before/after render counts and durations.
- **In CI**: Layer 1 tests run automatically via `just perf-ui` and will fail
  if render counts regress beyond the 2σ threshold stored in the baseline.

## Layer 1 — Component-Level Regression Tests (Vitest)

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

## Layer 2 — Interaction-Level Profiling (Playwright + React.Profiler)

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

## Ad-Hoc Profiling with React DevTools Exports

For investigating specific performance issues (e.g. "why does dragging this
slider feel sluggish?"), capture a profile from the React DevTools Profiler
and analyze it with the included script:

1. Open React DevTools > Profiler tab, record the interaction, then **Export**
   the profile as JSON.
2. Run:

```bash
just analyze-profile profiling-data.json           # summary + top components
just analyze-profile profiling-data.json --cascade  # re-render cascade trees
just analyze-profile profiling-data.json --why      # prop-change analysis
just analyze-profile profiling-data.json --commit 10  # single commit detail
just analyze-profile profiling-data.json --filter "Compositor|VideoLayer"
```

The script (`scripts/analyze-react-profile.mjs`) parses the React DevTools
profiling JSON (format v5), maps fiber IDs to component names, and reports:

- **Top components** by total self-time across all commits
- **Cascade trees** showing the full re-render chain for the heaviest commits
- **Why analysis** identifying prop-driven re-renders (potential object-reference
  instability) and cascade victims (components re-rendering only because a parent
  did)
- **Per-commit detail** with every component's self/actual time and trigger reason

Use this to pinpoint memoization breaks, unstable object references, and
unnecessary re-render cascades before diving into code.

## Updating the Baseline

Run `just perf-ui` — the last test in the render-perf suite writes a fresh
`perf-baselines.json` (gated behind `UPDATE_PERF_BASELINE=1`, which the
`test:perf` script sets automatically). Regular `just test-ui` runs compare
against the baseline but never overwrite it.

> **Important:** Never commit `perf-baselines.json` changes from agent sessions.
> Only human maintainers who have benchmarked on bare-metal idle systems should
> commit baseline updates. If `just perf-ui` modifies the file, revert it before
> committing.
