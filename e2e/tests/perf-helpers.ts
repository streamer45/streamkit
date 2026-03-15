// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Playwright helpers for Layer 2 render-performance profiling.
 *
 * These utilities interact with the dev-only `window.__PERF_DATA__` store
 * exposed by `ui/src/perf/profiler.ts`.  They allow Playwright tests to:
 *
 *   1. Reset profiling data before a scenario.
 *   2. Run an interaction (slider drag, button clicks, etc.).
 *   3. Capture a snapshot of render metrics.
 *   4. Compare against a previous snapshot or baseline.
 *
 * @example
 *   const before = await resetPerfData(page);
 *   await dragSlider(page, selector, 100);
 *   const snapshot = await capturePerfData(page);
 *   expectRenderCount(snapshot, 'CompositorNode', { max: 60 });
 */

import type { Page } from '@playwright/test';

// ── Types mirroring ui/src/perf/profiler.ts ──────────────────────────────────

export interface PerfCommit {
  id: string;
  phase: 'mount' | 'update' | 'nested-update';
  actualDuration: number;
  baseDuration: number;
  startTime: number;
  commitTime: number;
}

export interface PerfComponentData {
  renderCount: number;
  totalDuration: number;
  maxCommitDuration: number;
  commits: PerfCommit[];
}

export interface PerfSnapshot {
  components: Record<string, PerfComponentData>;
  session: number;
  startedAt: string;
}

// ── Core helpers ─────────────────────────────────────────────────────────────

/**
 * Reset the in-app perf profiler and return the (empty) initial state.
 * Must be called before the interaction you want to measure.
 */
export async function resetPerfData(page: Page): Promise<void> {
  await page.evaluate(() => {
    const w = window as Window & { __PERF_RESET__?: () => void };
    if (w.__PERF_RESET__) {
      w.__PERF_RESET__();
    } else {
      throw new Error('window.__PERF_RESET__ not found — is the app running in dev mode?');
    }
  });
}

/**
 * Capture the current perf data snapshot from the running app.
 */
export async function capturePerfData(page: Page): Promise<PerfSnapshot> {
  return page.evaluate(() => {
    const w = window as Window & { __PERF_DATA__?: PerfSnapshot };
    if (!w.__PERF_DATA__) {
      throw new Error('window.__PERF_DATA__ not found — is the app running in dev mode?');
    }
    // Deep clone to avoid stale references.
    return JSON.parse(JSON.stringify(w.__PERF_DATA__)) as PerfSnapshot;
  });
}

// ── Comparison utilities ─────────────────────────────────────────────────────

export interface RenderBudget {
  /** Maximum allowed render count.  Exceeding this fails the assertion. */
  max?: number;
  /** Maximum total duration in ms. */
  maxDuration?: number;
}

/**
 * Assert that a component's render metrics fall within the given budget.
 * Throws a descriptive error if the budget is exceeded.
 */
export function assertRenderBudget(
  snapshot: PerfSnapshot,
  componentId: string,
  budget: RenderBudget
): void {
  const data = snapshot.components[componentId];
  if (!data) {
    throw new Error(
      `No perf data for "${componentId}".  ` +
        `Available components: ${Object.keys(snapshot.components).join(', ') || '(none)'}`
    );
  }

  if (budget.max !== undefined && data.renderCount > budget.max) {
    throw new Error(
      `"${componentId}" rendered ${data.renderCount} times, ` + `exceeding budget of ${budget.max}.`
    );
  }

  if (budget.maxDuration !== undefined && data.totalDuration > budget.maxDuration) {
    throw new Error(
      `"${componentId}" total render duration was ${data.totalDuration.toFixed(1)}ms, ` +
        `exceeding budget of ${budget.maxDuration}ms.`
    );
  }
}

/**
 * Compare two perf snapshots and return per-component deltas.
 */
export function compareSnapshots(
  before: PerfSnapshot,
  after: PerfSnapshot
): Record<
  string,
  { renderCountDelta: number; durationDelta: number; renderCount: number; totalDuration: number }
> {
  const result: Record<
    string,
    { renderCountDelta: number; durationDelta: number; renderCount: number; totalDuration: number }
  > = {};

  for (const [id, afterData] of Object.entries(after.components)) {
    const beforeData = before.components[id];
    result[id] = {
      renderCount: afterData.renderCount,
      totalDuration: afterData.totalDuration,
      renderCountDelta: afterData.renderCount - (beforeData?.renderCount ?? 0),
      durationDelta: afterData.totalDuration - (beforeData?.totalDuration ?? 0),
    };
  }

  return result;
}

/**
 * Format a snapshot into a human-readable summary (for test output / CI logs).
 */
export function formatPerfSummary(snapshot: PerfSnapshot): string {
  const lines: string[] = [
    `Perf Snapshot (session ${snapshot.session}, started ${snapshot.startedAt})`,
    '-'.repeat(60),
  ];

  const entries = Object.entries(snapshot.components).sort(
    ([, a], [, b]) => b.renderCount - a.renderCount
  );

  for (const [id, data] of entries) {
    lines.push(
      `  ${id}: ${data.renderCount} renders, ` +
        `${data.totalDuration.toFixed(1)}ms total, ` +
        `${data.maxCommitDuration.toFixed(1)}ms max`
    );
  }

  if (entries.length === 0) {
    lines.push('  (no components profiled)');
  }

  return lines.join('\n');
}
