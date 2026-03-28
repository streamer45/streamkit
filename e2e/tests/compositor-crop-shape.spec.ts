// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * E2E tests for compositor crop shape controls (Rect / Circle).
 *
 * Creates a Webcam Circle PiP pipeline session via the API (with
 * crop_shape=circle on the PiP layer), navigates to the monitor view,
 * then exercises the crop shape segmented control:
 *
 * - Crop & Zoom section appears for video layers
 * - Shape segmented control shows Rect and Circle buttons
 * - Circle button is initially active (matches pipeline YAML)
 * - Clicking Rect switches the active state
 * - Clicking Circle switches back
 * - Canvas preview shows border-radius: 50% when circle is active
 * - Reset button restores shape to Rect
 * - Crop & Zoom section is NOT shown for text overlays
 */

import { test, expect, request, type Page, type Locator } from '@playwright/test';

import { ensureLoggedIn, getAuthHeaders } from './auth-helpers';
import {
  type ConsoleErrorCollector,
  MOQ_BENIGN_PATTERNS,
  createConsoleErrorCollector,
} from './test-helpers';
import { WEBCAM_PIP_CIRCLE_YAML } from './compositor-fixtures';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/**
 * Assert that a MirrorButton styled-component is active or inactive.
 *
 * Emotion does NOT forward the `isActive` prop to the DOM as an HTML
 * attribute — it only uses it to generate different CSS classes.  We
 * therefore check the computed background-color: an active button has a
 * non-transparent background, while an inactive one is transparent.
 */
async function expectButtonActive(button: Locator, active: boolean) {
  await expect(async () => {
    const bg = await button.evaluate((el) => window.getComputedStyle(el).backgroundColor);
    const isTransparent = bg === 'rgba(0, 0, 0, 0)' || bg === 'transparent' || bg === '';
    if (active) {
      expect(isTransparent, `Expected button to be active (non-transparent bg), got ${bg}`).toBe(
        false
      );
    } else {
      expect(isTransparent, `Expected button to be inactive (transparent bg), got ${bg}`).toBe(
        true
      );
    }
  }).toPass({ timeout: 3_000 });
}

async function setupCompositorView(page: Page) {
  await page.goto('/monitor');
  await ensureLoggedIn(page);
  if (!page.url().includes('/monitor')) {
    await page.goto('/monitor');
  }
  await expect(page.getByTestId('monitor-view')).toBeVisible({ timeout: 15_000 });

  await expect(page.getByTestId('sessions-list')).toBeVisible({ timeout: 10_000 });
  const sessionItem = page.getByTestId('session-item').first();
  await expect(sessionItem).toBeVisible({ timeout: 10_000 });
  await sessionItem.click();

  await expect(page.locator('.react-flow__node').first()).toBeVisible({ timeout: 15_000 });

  const compositorNode = page.locator('.react-flow__node').filter({ hasText: 'Compositor' });
  await expect(compositorNode).toBeVisible({ timeout: 10_000 });

  const canvasInner = compositorNode.locator('[data-canvas-width]');
  await expect(canvasInner).toBeVisible({ timeout: 5_000 });

  return { compositorNode, canvasInner };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

test.describe('Compositor Crop Shape Controls', () => {
  let collector: ConsoleErrorCollector;
  let sessionId: string | null = null;

  test.beforeEach(async ({ page }) => {
    collector = createConsoleErrorCollector(page);
  });

  test('crop shape segmented control toggles between Rect and Circle', async ({
    page,
    baseURL,
  }) => {
    test.setTimeout(120_000);

    // ── 1. Create session via API with circle crop on in_1 ───────────────

    const apiContext = await request.newContext({
      baseURL: baseURL!,
      extraHTTPHeaders: getAuthHeaders(),
    });

    const createResponse = await apiContext.post('/api/v1/sessions', {
      data: {
        name: `crop-shape-test-${Date.now()}`,
        yaml: WEBCAM_PIP_CIRCLE_YAML,
      },
    });

    const responseText = await createResponse.text();
    expect(createResponse.ok(), `Failed to create session: ${responseText}`).toBeTruthy();

    const createData = JSON.parse(responseText) as { session_id: string };
    sessionId = createData.session_id;
    expect(sessionId).toBeTruthy();
    await apiContext.dispose();

    // ── 2. Navigate to monitor view, find compositor node ────────────────

    const { compositorNode, canvasInner } = await setupCompositorView(page);

    // Verify expected layers are visible in the layer list.
    const inputLayer0 = compositorNode.getByText('Input 0', { exact: true }).first();
    const inputLayer1 = compositorNode.getByText('Input 1', { exact: true }).first();
    const textLayer = compositorNode.getByText('Text 0', { exact: true }).first();
    await expect(inputLayer0).toBeVisible({ timeout: 5_000 });
    await expect(inputLayer1).toBeVisible({ timeout: 5_000 });
    await expect(textLayer).toBeVisible({ timeout: 5_000 });

    // ── 3. Select video layer with circle crop ───────────────────────────

    await inputLayer1.click();

    const cropSection = compositorNode.getByTestId('crop-zoom-section');
    await expect(cropSection).toBeVisible({ timeout: 5_000 });

    // ── 4. Verify shape segmented control ────────────────────────────────

    const rectButton = compositorNode.getByTestId('crop-shape-rect');
    const circleButton = compositorNode.getByTestId('crop-shape-circle');
    await expect(rectButton).toBeVisible();
    await expect(circleButton).toBeVisible();

    // in_1 has crop_shape=circle in the pipeline YAML, so Circle should
    // be active initially.
    await expectButtonActive(circleButton, true);
    await expectButtonActive(rectButton, false);

    // ── 5. Verify canvas preview shows circular clip-path ─────────────────

    const videoLayerBox = canvasInner.locator('.nodrag.nopan').filter({ hasText: 'in_1' }).first();
    await expect(videoLayerBox).toBeVisible({ timeout: 5_000 });

    // clipPath lives on the inner [data-crop-circle] div, not the LayerBox
    // itself (the outer box stays unclipped so resize handles remain visible).
    const clipPath = await videoLayerBox.evaluate(
      (el) => {
        const inner = el.querySelector('[data-crop-circle]');
        return inner ? window.getComputedStyle(inner).clipPath : 'none';
      }
    );
    expect(clipPath).toMatch(/^circle\(/);

    // ── 6. Switch to Rect ────────────────────────────────────────────────

    await rectButton.click();

    await expectButtonActive(rectButton, true);
    await expectButtonActive(circleButton, false);

    // Canvas preview should no longer have circular clip-path.
    const clipPathAfterRect = await videoLayerBox.evaluate(
      (el) => {
        const inner = el.querySelector('[data-crop-circle]');
        return inner ? window.getComputedStyle(inner).clipPath : 'none';
      }
    );
    expect(clipPathAfterRect).toBe('none');

    // ── 7. Switch back to Circle ─────────────────────────────────────────

    await circleButton.click();

    await expectButtonActive(circleButton, true);

    const clipPathAfterCircle = await videoLayerBox.evaluate(
      (el) => {
        const inner = el.querySelector('[data-crop-circle]');
        return inner ? window.getComputedStyle(inner).clipPath : 'none';
      }
    );
    expect(clipPathAfterCircle).toMatch(/^circle\(/);

    // ── 8. Reset restores shape to Rect ──────────────────────────────────

    const resetButton = compositorNode.getByTestId('crop-zoom-reset');
    await expect(resetButton).toBeVisible();
    await resetButton.click();

    // After reset: shape should be Rect, zoom should be 1.0×.
    await expectButtonActive(rectButton, true);
    await expectButtonActive(circleButton, false);

    const zoomValue = compositorNode.getByTestId('crop-zoom-value');
    await expect(zoomValue).toHaveText('1.0×', { timeout: 3_000 });

    // Canvas preview should have no circular clip-path after reset.
    const clipPathAfterReset = await videoLayerBox.evaluate(
      (el) => {
        const inner = el.querySelector('[data-crop-circle]');
        return inner ? window.getComputedStyle(inner).clipPath : 'none';
      }
    );
    expect(clipPathAfterReset).toBe('none');

    // ── 9. Select text overlay — Crop & Zoom should NOT appear ───────────

    await textLayer.click();
    await expect(cropSection).not.toBeVisible({ timeout: 5_000 });

    // ── 10. Select in_0 — Crop & Zoom should appear with Rect default ────

    await inputLayer0.click();
    await expect(cropSection).toBeVisible({ timeout: 5_000 });

    // in_0 has no crop_shape set (defaults to rect).
    await expectButtonActive(rectButton, true);

    // ── 11. Console error check ──────────────────────────────────────────

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
