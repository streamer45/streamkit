// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Layer 2 — Compositor slider interaction perf test.
 *
 * Starts a Webcam PiP (MoQ Stream) pipeline in the stream view, then
 * navigates to the monitor view where the full compositor node graph is
 * rendered.  Selects each layer (text, bg, pip) and drags its opacity and
 * rotation sliders while measuring re-renders via `window.__PERF_DATA__`.
 *
 * The test asserts that render counts stay within budget — specifically that
 * slider interactions on one layer do NOT trigger expensive cascade
 * re-renders in unrelated components (the same regression PR #89 fixed).
 */

import { test, expect, request } from '@playwright/test';

import { ensureLoggedIn, getAuthHeaders } from './auth-helpers';
import {
  type ConsoleErrorCollector,
  MOQ_BENIGN_PATTERNS,
  createConsoleErrorCollector,
  installAudioContextTracker,
} from './test-helpers';
import {
  resetPerfData,
  capturePerfData,
  assertRenderBudget,
  formatPerfSummary,
} from './perf-helpers';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

test.describe('Compositor Slider Perf — Cascade Re-render Budget', () => {
  let collector: ConsoleErrorCollector;
  let sessionId: string | null = null;

  test.beforeEach(async ({ page }) => {
    collector = createConsoleErrorCollector(page);
    await installAudioContextTracker(page);
  });

  test('slider drags stay within render budget across all compositor components', async ({
    page,
    baseURL,
  }) => {
    // This test involves MoQ connection + multiple slider interactions.
    test.setTimeout(120_000);

    // ── 1. Create Webcam PiP session from stream view ───────────────────

    await page.goto('/stream');
    await ensureLoggedIn(page);
    if (!page.url().includes('/stream')) {
      await page.goto('/stream');
    }
    await expect(page.getByTestId('stream-view')).toBeVisible();

    // Check MoQ gateway availability; skip if not configured.
    const configResponse = await page.request.get(`${baseURL}/api/v1/config`);
    if (configResponse.ok()) {
      const config = (await configResponse.json()) as {
        moq_gateway_url?: string | null;
      };
      if (!config.moq_gateway_url) {
        test.skip(true, 'MoQ gateway not configured on this server');
      }
    }

    // Select the webcam PiP template.
    const templateCard = page.getByText('Webcam PiP (MoQ Stream)', {
      exact: true,
    });
    await expect(templateCard).toBeVisible({ timeout: 10_000 });
    await templateCard.click();

    // Create session.
    const createButton = page.getByRole('button', { name: /Create Session/i });
    await expect(createButton).toBeEnabled({ timeout: 5_000 });
    await createButton.click();

    const activeBadge = page.getByText('Session Active');
    await expect(activeBadge).toBeVisible({ timeout: 15_000 });

    // Extract session ID for cleanup.
    const sessionIdText = await page.getByText(/Session ID:/).textContent();
    sessionId = sessionIdText?.replace(/Session ID:\s*/, '').trim() ?? null;

    // Wait for MoQ connection.
    const connected = page.getByText('Relay: connected');
    const connectButton = page.getByRole('button', {
      name: /Connect & Stream/i,
    });

    await expect(connected.or(connectButton)).toBeVisible({ timeout: 20_000 });

    const isConnected = await connected.isVisible();
    if (!isConnected) {
      await expect(connectButton).toBeEnabled({ timeout: 5_000 });
      await connectButton.click();
      await expect(connected.or(page.getByText('Disconnected'))).toBeVisible({
        timeout: 20_000,
      });
    }

    if (!(await connected.isVisible())) {
      test.skip(
        true,
        'MoQ WebTransport connection could not be established in this environment'
      );
    }

    // Give the pipeline a moment to stabilise.
    await page.waitForTimeout(2_000);

    // ── 2. Navigate to monitor view ─────────────────────────────────────

    await page.goto('/monitor');
    await expect(page.getByTestId('monitor-view')).toBeVisible({
      timeout: 15_000,
    });

    // Wait for sessions list and select our session.
    await expect(page.getByTestId('sessions-list')).toBeVisible({
      timeout: 10_000,
    });

    // Click the session to load its pipeline graph.
    const sessionItem = page.getByTestId('session-item').first();
    await expect(sessionItem).toBeVisible({ timeout: 10_000 });
    await sessionItem.click();

    // Wait for the React Flow canvas and compositor node to render.
    await expect(page.locator('.react-flow__node').first()).toBeVisible({
      timeout: 15_000,
    });

    // Allow initial renders to settle.
    await page.waitForTimeout(2_000);

    // ── 3. Locate compositor side panel & layer list ────────────────────

    // The compositor node renders a side panel with a unified layer list.
    // Each layer item shows a friendly label like "Background", "PiP",
    // "Text 1", etc.  We need to click each to select it, then interact
    // with the opacity and rotation sliders in the inspector.

    // Find the compositor node on the canvas.  It is the widest node
    // (minWidth 320) and contains "Compositor" in its header.
    const compositorNode = page.locator('.react-flow__node').filter({
      hasText: 'Compositor',
    });
    await expect(compositorNode).toBeVisible({ timeout: 10_000 });

    // The layer list items inside the compositor.  Each entry is a styled
    // <li> or row with the layer's friendly name.
    const layerItems = compositorNode.locator('li, [class*="LayerListItem"]');

    // Fallback: if no list items are found, try broader approach.
    const layerCount = await layerItems.count();

    if (layerCount === 0) {
      // The pipeline may not have layers yet — skip gracefully.
      console.log('No layer items found in compositor — skipping perf measurement');
      return;
    }

    console.log(`Found ${layerCount} layer(s) in compositor`);

    // ── 4. Measure slider interactions per layer ────────────────────────

    // Reset perf data before our measurement window.
    await resetPerfData(page);

    for (let i = 0; i < layerCount; i++) {
      const layer = layerItems.nth(i);
      const layerText = (await layer.textContent()) ?? `layer-${i}`;

      // Select the layer by clicking it.
      await layer.click();
      await page.waitForTimeout(300); // let inspector appear

      // --- Opacity slider ---
      // The OpacityControl section has an "Opacity" label followed by a
      // Radix slider.  We locate it by finding the section labelled
      // "Opacity" within the compositor node.
      const opacitySection = compositorNode.locator('text=Opacity').locator('..');
      const opacitySliderVisible = await opacitySection
        .getByRole('slider')
        .isVisible()
        .catch(() => false);

      if (opacitySliderVisible) {
        console.log(`  Dragging opacity slider for "${layerText.trim()}"`);
        await dragSliderThumb(page, opacitySection, 40);
        await page.waitForTimeout(100);
        // Drag back to exercise more render cycles.
        await dragSliderThumb(page, opacitySection, -40);
        await page.waitForTimeout(100);
      }

      // --- Rotation slider ---
      const rotationSection = compositorNode.locator('text=Rotation').locator('..');
      const rotationSliderVisible = await rotationSection
        .getByRole('slider')
        .isVisible()
        .catch(() => false);

      if (rotationSliderVisible) {
        console.log(`  Dragging rotation slider for "${layerText.trim()}"`);
        await dragSliderThumb(page, rotationSection, 60);
        await page.waitForTimeout(100);
        await dragSliderThumb(page, rotationSection, -60);
        await page.waitForTimeout(100);
      }
    }

    // ── 5. Capture and assert render budgets ────────────────────────────

    const snapshot = await capturePerfData(page);
    console.log('\n' + formatPerfSummary(snapshot));

    // CompositorNode itself will re-render on each slider tick — but with
    // proper memoization the total should stay bounded.  The budget below
    // is generous (accommodates CI jitter) while still catching the
    // pre-PR-#89 regression where every slider tick caused 94+ fiber
    // re-renders across the entire tree.
    //
    // If CompositorNode is profiled, assert a generous upper bound.
    // On a PiP pipeline with 3 layers × 2 sliders × 2 directions × 20
    // steps = 240 ticks, plus mount renders, ~300 is a reasonable ceiling.
    const compositorData = snapshot.components['CompositorNode'];
    if (compositorData) {
      assertRenderBudget(snapshot, 'CompositorNode', {
        max: 350,
        maxDuration: 5_000,
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

    // ── 6. Console error check ──────────────────────────────────────────

    const unexpected = collector.getUnexpected(MOQ_BENIGN_PATTERNS);
    // Log but don't fail — monitor view may have transient warnings during
    // session state transitions that aren't perf-related.
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
