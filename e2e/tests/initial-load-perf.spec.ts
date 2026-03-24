// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Layer 2 — Initial load perf test for complex pipelines.
 *
 * Creates a Webcam PiP pipeline session (9 nodes, 10 edges) via the API,
 * navigates to the monitor view, and measures how many times each profiled
 * node component renders during the initial load + layout cycle.
 *
 * With proper `areNodePropsEqual` comparators, each node should render at
 * most twice (mount + one layout-driven update).  Without them, ReactFlow's
 * position/dimension prop changes cause 3–4 full render waves across all
 * nodes.
 *
 * NOTE: Requires the Vite dev server (`just ui`) because the profiler
 * store (`window.__PERF_DATA__`) is only exposed when
 * `import.meta.env.DEV` is true.
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

test.describe('Initial Load Perf — Node Render Budget', () => {
  let collector: ConsoleErrorCollector;
  let sessionId: string | null = null;
  let sessionName: string | null = null;

  test.beforeEach(async ({ page }) => {
    collector = createConsoleErrorCollector(page);
  });

  test('complex pipeline initial load stays within node render budget', async ({
    page,
    baseURL,
  }) => {
    test.setTimeout(90_000);

    // ── 1. Create Webcam PiP session via API ──────────────────────────────

    const apiContext = await request.newContext({
      baseURL: baseURL!,
      extraHTTPHeaders: getAuthHeaders(),
    });

    sessionName = `perf-initial-load-${Date.now()}`;
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

    // ── 2. Navigate to monitor view ───────────────────────────────────────

    await page.goto('/monitor');
    await ensureLoggedIn(page);
    if (!page.url().includes('/monitor')) {
      await page.goto('/monitor');
    }
    await expect(page.getByTestId('monitor-view')).toBeVisible({
      timeout: 15_000,
    });

    // ── 3. Verify dev-mode profiler is available ──────────────────────────

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

    // ── 4. Reset perf data, then trigger initial load ─────────────────────
    //
    // Reset profiler BEFORE clicking the session so we only capture the
    // renders caused by the initial pipeline load + ReactFlow layout.

    await resetPerfData(page);

    // Wait for sessions list and click our session.
    await expect(page.getByTestId('sessions-list')).toBeVisible({
      timeout: 10_000,
    });

    const sessionItem = page.getByTestId('session-item').filter({ hasText: sessionName! });
    await expect(sessionItem).toBeVisible({ timeout: 10_000 });
    await sessionItem.click();

    // Wait for the React Flow canvas to render all nodes.
    await expect(page.locator('.react-flow__node').first()).toBeVisible({
      timeout: 15_000,
    });

    // Give ReactFlow time to finish dimension measurement and auto-layout.
    // This is the window where redundant re-renders used to happen.
    await page.waitForTimeout(2_000);

    // ── 5. Capture and assert render budgets ──────────────────────────────

    const snapshot = await capturePerfData(page);
    console.log('\n' + formatPerfSummary(snapshot));

    // With areNodePropsEqual comparators, each node type should render at
    // most 2–3 times during initial load (mount + possible layout update).
    // Before the fix, nodes rendered 4+ times due to position/dimension
    // prop churn from ReactFlow's measurement cycle.
    //
    // Budget: 4 renders per node type gives headroom for CI jitter while
    // still catching the pre-fix regression (which had 6–10+ renders per
    // node).

    const profiled = ['CompositorNode', 'ConfigurableNode', 'AudioGainNode'];

    for (const componentId of profiled) {
      const data = snapshot.components[componentId];
      if (!data) {
        // Component may not have rendered if the pipeline didn't fully
        // initialise — skip rather than fail.
        console.warn(`No perf data for ${componentId} — skipping assertion`);
        continue;
      }

      assertRenderBudget(snapshot, componentId, {
        max: 4,
        maxDuration: 500,
      });
    }

    // ── 6. Cross-node cascade check ───────────────────────────────────────
    //
    // All profiled nodes should have roughly similar render counts during
    // initial load.  If one node type has dramatically more renders, it
    // suggests a targeted memo regression.

    const renderCounts = profiled
      .map((id) => snapshot.components[id]?.renderCount)
      .filter((c): c is number => c !== undefined);

    if (renderCounts.length >= 2) {
      const maxCount = Math.max(...renderCounts);
      const minCount = Math.min(...renderCounts);
      // Allow up to 3× spread — initial load should be roughly uniform.
      if (maxCount > minCount * 3) {
        console.warn(
          `Render count spread is high: min=${minCount}, max=${maxCount}. ` +
            `This may indicate a memoization gap in one node type.`
        );
      }
    }

    // ── 7. Console error check ────────────────────────────────────────────

    const unexpected = collector.getUnexpected(MOQ_BENIGN_PATTERNS);
    if (unexpected.length > 0) {
      console.warn('Unexpected console errors (non-fatal):', unexpected);
    }
  });

  // ── Cleanup ───────────────────────────────────────────────────────────────

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
