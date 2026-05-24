// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Layer 2 — Compositor slider interaction perf test.
 *
 * Creates a Webcam PiP pipeline session via the API, navigates to the monitor
 * view where the full compositor node graph is rendered, then selects each
 * layer and drags its opacity and rotation sliders while measuring re-renders
 * via `window.__PERF_DATA__`.
 *
 * The test asserts that render counts stay within budget — specifically that
 * slider interactions on one layer do NOT trigger expensive cascade
 * re-renders in unrelated components (the same regression PR #89 fixed).
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

/**
 * Simulate dragging a Radix slider thumb horizontally by `deltaX` pixels.
 * The thumb is located via its `role="slider"` within the given container.
 */
async function dragSliderThumb(
  page: import('@playwright/test').Page,
  container: import('@playwright/test').Locator,
  deltaX: number
) {
  const thumb = container.getByRole('slider');
  await thumb.waitFor({ state: 'visible', timeout: 5_000 });
  const box = await thumb.boundingBox();
  if (!box) throw new Error('Slider thumb has no bounding box');

  const startX = box.x + box.width / 2;
  const startY = box.y + box.height / 2;

  await page.mouse.move(startX, startY);
  await page.mouse.down();

  // Move in small increments to simulate a realistic drag that fires
  // multiple onValueChange events.
  const steps = 20;
  const stepSize = deltaX / steps;
  for (let i = 1; i <= steps; i++) {
    await page.mouse.move(startX + stepSize * i, startY);
  }

  await page.mouse.up();
}

test.describe('Compositor Slider Perf — Cascade Re-render Budget', () => {
  let collector: ConsoleErrorCollector;
  let sessionId: string | null = null;
  let sessionName: string | null = null;

  test.beforeEach(async ({ page }) => {
    collector = createConsoleErrorCollector(page);
  });

  test('slider drags stay within render budget across all compositor components', async ({
    page,
    baseURL,
  }) => {
    // This test involves API session creation + multiple slider interactions.
    test.setTimeout(120_000);
    //
    // Using the API avoids the stream view flow and MoQ WebTransport
    // connection, which is unreliable in headless CI environments.

    const apiContext = await request.newContext({
      baseURL: baseURL!,
      extraHTTPHeaders: getAuthHeaders(),
    });

    sessionName = `perf-test-${Date.now()}`;
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

    await page.goto('/monitor');
    await ensureLoggedIn(page);
    if (!page.url().includes('/monitor')) {
      await page.goto('/monitor');
    }
    await expect(page.getByTestId('monitor-view')).toBeVisible({
      timeout: 15_000,
    });

    // Wait for sessions list and click our session by name.
    await expect(page.getByTestId('sessions-list')).toBeVisible({
      timeout: 10_000,
    });

    const sessionItem = page.getByTestId('session-item').filter({ hasText: sessionName! });
    await expect(sessionItem).toBeVisible({ timeout: 10_000 });
    await sessionItem.click();

    // Wait for the React Flow canvas and compositor node to render.
    await expect(page.locator('.react-flow__node').first()).toBeVisible({
      timeout: 15_000,
    });

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

    // The compositor node is the React Flow node containing "Compositor".
    // Wait for graph to settle before measuring perf data.
    const compositorNode = page.locator('.react-flow__node').filter({
      hasText: 'Compositor',
    });
    await expect(compositorNode).toBeVisible({ timeout: 10_000 });

    // Layer names in the PiP pipeline: "Text 0", "Input 1", "Input 0".
    // These are plain <div> elements inside the layer list — no test IDs
    // or <li> wrappers.  We locate them by exact text content within the
    // compositor node.
    const layerNames = ['Text 0', 'Input 1', 'Input 0'];
    const availableLayers: string[] = [];

    for (const name of layerNames) {
      const layerDiv = compositorNode.getByText(name, { exact: true });
      if (
        await layerDiv
          .first()
          .isVisible()
          .catch(() => false)
      ) {
        availableLayers.push(name);
      }
    }

    if (availableLayers.length === 0) {
      test.skip(true, 'No compositor layers found — pipeline may not have initialised');
    }

    console.log(`Found ${availableLayers.length} layer(s): ${availableLayers.join(', ')}`);

    // Reset perf data before our measurement window.
    await resetPerfData(page);

    for (const layerName of availableLayers) {
      // Click the layer in the layer list to select it and open inspector.
      const layerDiv = compositorNode.getByText(layerName, { exact: true });
      await layerDiv.first().click();
      // Wait for the inspector panel (with sliders) to become visible.
      await expect(compositorNode.getByRole('slider').first()).toBeVisible({ timeout: 3_000 });
      // The inspector shows an "Opacity" label followed by a Radix slider
      // (role="slider").  We locate the innermost div containing "Opacity"
      // that also holds a slider thumb.
      const opacitySection = compositorNode
        .locator('div')
        .filter({ hasText: /^Opacity/ })
        .filter({ has: page.getByRole('slider') })
        .first();

      const hasOpacity = await opacitySection
        .getByRole('slider')
        .isVisible()
        .catch(() => false);

      if (hasOpacity) {
        console.log(`  Dragging opacity slider for "${layerName}"`);
        await dragSliderThumb(page, opacitySection, 40);
        await page.waitForTimeout(100);
        // Drag back to exercise more render cycles.
        await dragSliderThumb(page, opacitySection, -40);
        await page.waitForTimeout(100);
      }
      // Similar approach: find the section labelled "Rotation" that
      // contains a slider.  (Rotation also has preset buttons like
      // 0°/90°/180°/270° but we specifically target the slider.)
      const rotationSection = compositorNode
        .locator('div')
        .filter({ hasText: /^Rotation/ })
        .filter({ has: page.getByRole('slider') })
        .first();

      const hasRotation = await rotationSection
        .getByRole('slider')
        .isVisible()
        .catch(() => false);

      if (hasRotation) {
        console.log(`  Dragging rotation slider for "${layerName}"`);
        await dragSliderThumb(page, rotationSection, 60);
        await page.waitForTimeout(100);
        await dragSliderThumb(page, rotationSection, -60);
        await page.waitForTimeout(100);
      }
    }

    const snapshot = await capturePerfData(page);
    console.log('\n' + formatPerfSummary(snapshot));

    // CompositorNode itself will re-render on each slider tick — but with
    // proper memoization the total should stay bounded.  The budget below
    // is generous (accommodates CI jitter) while still catching the
    // pre-PR-#89 regression where every slider tick caused 94+ fiber
    // re-renders across the entire tree.
    //
    // Observed baseline: ~440 renders / ~5800ms for the full 3-layer
    // scenario (after crop-shape state was added to video layers).
    // Echo-backs are skipped during slider drags (fire-and-forget with
    // reconciliation on commit), keeping the count well bounded.
    // Budget of 550 renders / 7500ms gives ~25% headroom while still
    // catching cascade regressions.
    const compositorData = snapshot.components['CompositorNode'];
    if (compositorData) {
      assertRenderBudget(snapshot, 'CompositorNode', {
        max: 550,
        maxDuration: 7_500,
      });
    }

    // The key cascade assertion: if there are OTHER profiled components
    // (siblings rendered outside the active slider path), their render
    // count should be dramatically lower than CompositorNode's.  A cascade
    // regression would show them rendering at a similar rate.
    for (const [id, data] of Object.entries(snapshot.components)) {
      if (id === 'CompositorNode') continue;
      // Sibling components should not exceed a fraction of the compositor's
      // render count — generous 60% ceiling to allow for legitimate
      // re-renders while catching full-cascade regressions.
      if (compositorData && data.renderCount > compositorData.renderCount * 0.6) {
        throw new Error(
          `Cascade regression detected: "${id}" rendered ${data.renderCount} times ` +
            `(${((data.renderCount / compositorData.renderCount) * 100).toFixed(0)}% of CompositorNode's ` +
            `${compositorData.renderCount}). This suggests slider interactions are causing ` +
            `expensive re-renders in unrelated components.`
        );
      }
    }

    const unexpected = collector.getUnexpected(MOQ_BENIGN_PATTERNS);
    // Log but don't fail — monitor view may have transient warnings during
    // session state transitions that aren't perf-related.
    if (unexpected.length > 0) {
      console.warn('Unexpected console errors (non-fatal):', unexpected);
    }
  });

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
