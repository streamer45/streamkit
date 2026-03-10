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

// ---------------------------------------------------------------------------
// Pipeline YAML — Webcam PiP compositor
// Embedded so the test is self-contained and does not depend on file paths.
// ---------------------------------------------------------------------------

const WEBCAM_PIP_YAML = `
name: Webcam PiP (MoQ Stream)
description: Composites the user's webcam as picture-in-picture over colorbars with a text overlay
mode: dynamic

nodes:
  colorbars_bg:
    kind: video::colorbars
    params:
      width: 1280
      height: 720
      fps: 30
      draw_time: true

  moq_peer:
    kind: transport::moq::peer
    params:
      gateway_path: /moq/video
      input_broadcast: input
      output_broadcast: output
      allow_reconnect: true
    needs:
      in: opus_encoder
      in_1: vp9_encoder

  vp9_decoder:
    kind: video::vp9::decoder
    needs:
      in: moq_peer.out_1

  compositor:
    kind: video::compositor
    params:
      width: 1280
      height: 720
      num_inputs: 2
      layers:
        in_0:
          opacity: 1.0
          z_index: 0
        in_1:
          rect:
            x: 880
            y: 20
            width: 380
            height: 285
          opacity: 0.95
          z_index: 1
      text_overlays:
        - text: "Hello from StreamKit"
          rect:
            x: 40
            y: 660
            width: 400
            height: 40
          opacity: 1.0
          z_index: 2
          color: [255, 255, 255, 220]
          font_size: 28
          font_name: dejavu-sans-bold
    needs:
      - colorbars_bg
      - vp9_decoder

  pixel_convert:
    kind: video::pixel_convert
    params:
      output_format: nv12
    needs: compositor

  vp9_encoder:
    kind: video::vp9::encoder
    params:
      keyframe_interval: 30
    needs: pixel_convert

  opus_decoder:
    kind: audio::opus::decoder
    needs: moq_peer

  gain:
    kind: audio::gain
    params:
      gain: 1.0
    needs: opus_decoder

  opus_encoder:
    kind: audio::opus::encoder
    needs: gain
`.trim();

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
  });

  test('slider drags stay within render budget across all compositor components', async ({
    page,
    baseURL,
  }) => {
    // This test involves API session creation + multiple slider interactions.
    test.setTimeout(120_000);

    // ── 1. Create Webcam PiP session via API ────────────────────────────
    //
    // Using the API avoids the stream view flow and MoQ WebTransport
    // connection, which is unreliable in headless CI environments.

    const apiContext = await request.newContext({
      baseURL: baseURL!,
      extraHTTPHeaders: getAuthHeaders(),
    });

    const createResponse = await apiContext.post('/api/v1/sessions', {
      data: {
        name: `perf-test-${Date.now()}`,
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

    // Wait for sessions list and click our session.
    await expect(page.getByTestId('sessions-list')).toBeVisible({
      timeout: 10_000,
    });

    const sessionItem = page.getByTestId('session-item').first();
    await expect(sessionItem).toBeVisible({ timeout: 10_000 });
    await sessionItem.click();

    // Wait for the React Flow canvas and compositor node to render.
    await expect(page.locator('.react-flow__node').first()).toBeVisible({
      timeout: 15_000,
    });

    // Allow initial renders to settle.
    await page.waitForTimeout(2_000);

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

    // ── 4. Locate compositor node and its layer list ────────────────────

    // The compositor node is the React Flow node containing "Compositor".
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

    // ── 5. Measure slider interactions per layer ────────────────────────

    // Reset perf data before our measurement window.
    await resetPerfData(page);

    for (const layerName of availableLayers) {
      // Click the layer in the layer list to select it and open inspector.
      const layerDiv = compositorNode.getByText(layerName, { exact: true });
      await layerDiv.first().click();
      await page.waitForTimeout(500); // let inspector render

      // --- Opacity slider ---
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

      // --- Rotation slider ---
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

    // ── 6. Capture and assert render budgets ────────────────────────────

    const snapshot = await capturePerfData(page);
    console.log('\n' + formatPerfSummary(snapshot));

    // CompositorNode itself will re-render on each slider tick — but with
    // proper memoization the total should stay bounded.  The budget below
    // is generous (accommodates CI jitter) while still catching the
    // pre-PR-#89 regression where every slider tick caused 94+ fiber
    // re-renders across the entire tree.
    //
    // Observed baseline: ~500 renders for the full 3-layer scenario
    // (originally ~385 when the test was written; drifted upward due to
    // additional inspector controls and server echo-back timing).
    // Budget of 600 gives ~20% headroom while still catching the
    // pre-PR-#89 regression (thousands of renders when every slider
    // tick triggered 94+ fiber re-renders).
    const compositorData = snapshot.components['CompositorNode'];
    if (compositorData) {
      assertRenderBudget(snapshot, 'CompositorNode', {
        max: 600,
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

    // ── 7. Console error check ──────────────────────────────────────────

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
