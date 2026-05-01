// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Layer 2 — Monitor view session-load perf test.
 *
 * Creates a multi-node pipeline session via the API, navigates to the monitor
 * view, selects the session, and measures re-renders that occur during the
 * initial session load (pipeline hydration + WebSocket state events).
 *
 * The test asserts that:
 *   - MonitorView renders stay within a reasonable budget during session load.
 *   - The batching optimisations prevent cascade re-renders from individual
 *     WebSocket events flooding the Zustand store with separate set() calls.
 *
 * NOTE: This test requires the Vite dev server (`just ui`) because the
 * profiler store (`window.__PERF_DATA__`) is only exposed when
 * `import.meta.env.DEV` is true.  Point E2E_BASE_URL at
 * http://localhost:3045 (or wherever the dev server runs).
 */

import { test, expect, request } from '@playwright/test';

import { ensureLoggedIn, getAuthHeaders } from './auth-helpers';
import {
  type ConsoleErrorCollector,
  MOQ_BENIGN_PATTERNS,
  createConsoleErrorCollector,
} from './test-helpers';
import {
  resetPerfData,
  capturePerfData,
  assertRenderBudget,
  formatPerfSummary,
} from './perf-helpers';
import { WEBCAM_PIP_YAML } from './compositor-fixtures';

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

test.describe('Monitor Session Load Perf — Re-render Budget', () => {
  let collector: ConsoleErrorCollector;
  let sessionId: string | null = null;

  test.beforeEach(async ({ page }) => {
    collector = createConsoleErrorCollector(page);
  });

  test('session load stays within render budget', async ({ page, baseURL }) => {
    // Session creation + full load + settle time.
    test.setTimeout(90_000);

    // ── 1. Create a multi-node session via API ──────────────────────────
    //
    // The Webcam PiP pipeline has ~10 nodes which exercises the session
    // load path well — each node will fire WebSocket state events that
    // our batching should coalesce.

    const apiContext = await request.newContext({
      baseURL: baseURL!,
      extraHTTPHeaders: getAuthHeaders(),
    });

    const sessionName = `perf-monitor-load-${Date.now()}`;
    const createResponse = await apiContext.post('/api/v1/sessions', {
      data: {
        name: sessionName,
        yaml: WEBCAM_PIP_YAML,
      },
    });

    const responseText = await createResponse.text();
    expect(createResponse.ok(), `Failed to create session: ${responseText}`).toBeTruthy();

    const createData = JSON.parse(responseText) as { session_id: string };
    sessionId = createData.session_id;
    expect(sessionId).toBeTruthy();
    await apiContext.dispose();

    // ── 2. Navigate to monitor view ─────────────────────────────────────

    await page.goto('/monitor');
    await ensureLoggedIn(page);
    if (!page.url().includes('/monitor')) {
      await page.goto('/monitor');
    }
    await expect(page.getByTestId('monitor-view')).toBeVisible({
      timeout: 15_000,
    });

    // Wait for sessions list to appear.
    await expect(page.getByTestId('sessions-list')).toBeVisible({
      timeout: 10_000,
    });

    // ── 3. Verify dev-mode profiler is available ────────────────────────

    const hasPerfData = await page.evaluate(() => {
      const w = window as Window & {
        __PERF_DATA__?: unknown;
        __PERF_RESET__?: unknown;
      };
      return !!w.__PERF_DATA__ && !!w.__PERF_RESET__;
    });

    if (!hasPerfData) {
      test.skip(
        true,
        'window.__PERF_DATA__ not found — test requires the Vite dev server (just ui)'
      );
    }

    // ── 4. Reset perf data, then trigger session load ───────────────────
    //
    // MonitorView auto-selects the first session on mount, so by the time
    // we reach this point the session may already be selected and its
    // nodes rendered — meaning a click would be a no-op and the profiler
    // would capture zero components.
    //
    // To isolate session-load renders reliably:
    //   1. Navigate away to unmount MonitorView (clears selection state).
    //   2. Reset the profiler so it starts with a clean slate.
    //   3. Navigate back to /monitor — auto-selection fires and renders
    //      all nodes fresh, with the profiler capturing every commit.

    await page.goto('/design');
    await expect(page.getByTestId('design-view')).toBeVisible({
      timeout: 10_000,
    });

    await resetPerfData(page);

    await page.goto('/monitor');
    await expect(page.getByTestId('monitor-view')).toBeVisible({
      timeout: 15_000,
    });

    // ── 5. Wait for the session graph to fully load ─────────────────────
    //
    // Wait for React Flow nodes to appear — this signals that the
    // pipeline has been hydrated and rendered via auto-selection.

    await expect(page.locator('.react-flow__node').first()).toBeVisible({
      timeout: 15_000,
    });

    // Wait for render counts to stabilise.  Instead of a fixed delay,
    // poll until two consecutive snapshots report the same total commit
    // count — this is both faster on quick machines and more reliable on
    // slow CI runners than a hard `waitForTimeout`.
    const POLL_INTERVAL = 300;
    const STABLE_THRESHOLD = 2; // consecutive stable readings required
    const POLL_TIMEOUT = 10_000;
    let stableCount = 0;
    let prevTotal = -1;
    const pollStart = Date.now();
    while (stableCount < STABLE_THRESHOLD && Date.now() - pollStart < POLL_TIMEOUT) {
      await page.waitForTimeout(POLL_INTERVAL);
      const snap = await capturePerfData(page);
      const total = Object.values(snap.components).reduce(
        (sum, c) => sum + (c as { renderCount: number }).renderCount,
        0
      );
      if (total === prevTotal) {
        stableCount++;
      } else {
        stableCount = 0;
        prevTotal = total;
      }
    }

    // ── 6. Capture and assert render budgets ────────────────────────────

    const snapshot = await capturePerfData(page);
    console.log('\n' + formatPerfSummary(snapshot));

    // The profiler store is available (dev mode).  We verify the perf
    // infrastructure works and that the session load path completes
    // without hanging or crashing.
    //
    // Render-budget gates: catch regressions in ConfigurableNode renders
    // during session load.  Node components now read state/params from
    // per-node Jotai atoms instead of ReactFlow data props, so a state
    // change on one node no longer forces every node to re-render.
    const configurableData = snapshot.components['ConfigurableNode'];
    expect(
      configurableData,
      'ConfigurableNode profiler data must be present — ensure the Profiler wrapper is intact'
    ).toBeDefined();
    assertRenderBudget(snapshot, 'ConfigurableNode', {
      max: 25,
      maxDuration: 1_500,
    });

    const compositorData = snapshot.components['CompositorNode'];
    if (compositorData) {
      assertRenderBudget(snapshot, 'CompositorNode', {
        max: 150,
        maxDuration: 3_000,
      });
    }

    // Regardless of which profiler IDs are present, verify that the perf
    // snapshot captured at least one component — proof that the profiler
    // infrastructure ran during the session load.
    expect(
      Object.keys(snapshot.components).length,
      'Perf snapshot should contain at least one profiled component after session load'
    ).toBeGreaterThan(0);

    // ── 7. Console error check ──────────────────────────────────────────

    const unexpected = collector.getUnexpected(MOQ_BENIGN_PATTERNS);
    if (unexpected.length > 0) {
      console.warn('Unexpected console errors (non-fatal):', unexpected);
    }
  });

  // ── Cleanup ─────────────────────────────────────────────────────────────

  test.afterEach(async ({ baseURL }) => {
    if (sessionId) {
      try {
        const apiContext = await request.newContext({
          baseURL: baseURL!,
          extraHTTPHeaders: getAuthHeaders(),
        });
        await apiContext.delete(`/api/v1/sessions/${sessionId}`);
        await apiContext.dispose();
      } catch {
        // Best-effort cleanup; ignore errors.
      }
      sessionId = null;
    }
  });
});
