// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Runtime render-performance profiler for Layer 2 (Playwright integration).
 *
 * Collects React.Profiler `onRender` data into a global store that Playwright
 * tests can read via `page.evaluate(() => window.__PERF_DATA__)`.
 *
 * In production builds this module is a no-op: all functions return immediately
 * and no data is stored.
 *
 * Usage in a React component (dev only):
 *
 *   import { perfOnRender } from '@/perf/profiler';
 *   <Profiler id="CompositorNode" onRender={perfOnRender}>
 *     <CompositorNodeInner />
 *   </Profiler>
 *
 * Reading from Playwright:
 *
 *   const data = await page.evaluate(() => window.__PERF_DATA__);
 */

/** @public — consumed by Playwright e2e tests via window.__PERF_DATA__ */
export interface PerfCommit {
  id: string;
  phase: 'mount' | 'update' | 'nested-update';
  actualDuration: number;
  baseDuration: number;
  startTime: number;
  commitTime: number;
}

/** @public — consumed by Playwright e2e tests via window.__PERF_DATA__ */
export interface PerfComponentData {
  /** Total number of commits for this profiler id. */
  renderCount: number;
  /** Sum of actualDuration across all commits. */
  totalDuration: number;
  /** Maximum single-commit actualDuration. */
  maxCommitDuration: number;
  /** Per-commit records (capped to avoid memory bloat). */
  commits: PerfCommit[];
}

/** @public — consumed by Playwright e2e tests via window.__PERF_DATA__ */
export interface PerfDataStore {
  /** Component-level profiling data keyed by Profiler id. */
  components: Record<string, PerfComponentData>;
  /** Monotonic session counter — incremented on each reset(). */
  session: number;
  /** ISO timestamp of when the current session started. */
  startedAt: string;
}

/** Maximum number of individual commit records stored per component. */
const MAX_COMMITS = 500;

const isDev = import.meta.env.DEV;

function createStore(): PerfDataStore {
  return {
    components: {},
    session: 0,
    startedAt: new Date().toISOString(),
  };
}

const store: PerfDataStore = createStore();

// Expose on window in dev mode so Playwright can read it.
if (isDev && typeof window !== 'undefined') {
  (window as Window & { __PERF_DATA__?: PerfDataStore }).__PERF_DATA__ = store;
}

/**
 * React.Profiler `onRender` callback.  Pass this directly:
 *
 *   <Profiler id="MyComponent" onRender={perfOnRender}>
 */
export const perfOnRender: React.ProfilerOnRenderCallback = isDev
  ? (
      id: string,
      phase: 'mount' | 'update' | 'nested-update',
      actualDuration: number,
      baseDuration: number,
      startTime: number,
      commitTime: number
    ) => {
      let entry = store.components[id];
      if (!entry) {
        entry = {
          renderCount: 0,
          totalDuration: 0,
          maxCommitDuration: 0,
          commits: [],
        };
        store.components[id] = entry;
      }

      entry.renderCount += 1;
      entry.totalDuration += actualDuration;
      entry.maxCommitDuration = Math.max(entry.maxCommitDuration, actualDuration);

      if (entry.commits.length < MAX_COMMITS) {
        entry.commits.push({
          id,
          phase,
          actualDuration,
          baseDuration,
          startTime,
          commitTime,
        });
      }
    }
  : // No-op in production.
    () => {};

/**
 * Reset all collected data and start a new measurement session.
 * Call this from Playwright before running an interaction scenario:
 *
 *   await page.evaluate(() => window.__PERF_RESET__?.());
 *
 * @public — consumed by Playwright e2e tests
 */
export function resetPerfData(): void {
  if (!isDev) return;
  store.session += 1;
  store.components = {};
  store.startedAt = new Date().toISOString();
  if (typeof window !== 'undefined') {
    (window as Window & { __PERF_DATA__?: PerfDataStore }).__PERF_DATA__ = store;
  }
}

// Also expose reset on window for Playwright access.
if (isDev && typeof window !== 'undefined') {
  (window as Window & { __PERF_RESET__?: () => void }).__PERF_RESET__ = resetPerfData;
}

/**
 * Get a snapshot of the current perf data (for programmatic use in tests).
 *
 * @public — consumed by Playwright e2e tests
 */
export function getPerfData(): PerfDataStore {
  return store;
}
