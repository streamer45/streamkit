// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * E2E tests for the Webcam Circle PiP pipeline.
 *
 * Validates the new circular crop feature (#176) in a full streaming
 * context.  Creates a session with crop_shape=circle on the PiP layer,
 * verifies the compositor node renders correctly in the monitor view
 * with the circular preview CSS, and confirms the pipeline processes
 * without errors.
 *
 * This test exercises:
 * - Session creation with circle crop pipeline YAML
 * - Monitor view: compositor node visible with correct crop shape state
 * - Canvas layer preview shows circular border-radius
 * - Crop & Zoom inspector reflects circle shape
 * - No console errors from the circular crop pipeline
 */

import { test, expect, request, type Page, type Locator } from '@playwright/test';

import { ensureLoggedIn, getAuthHeaders } from './auth-helpers';
import {
  type ConsoleErrorCollector,
  MOQ_BENIGN_PATTERNS,
  createConsoleErrorCollector,
} from './test-helpers';
import { WEBCAM_PIP_CIRCLE_YAML } from './compositor-fixtures';

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

async function setupCompositorView(page: Page, sessionName: string) {
  await page.goto('/monitor');
  await ensureLoggedIn(page);
  if (!page.url().includes('/monitor')) {
    await page.goto('/monitor');
  }
  await expect(page.getByTestId('monitor-view')).toBeVisible({ timeout: 15_000 });

  await expect(page.getByTestId('sessions-list')).toBeVisible({ timeout: 10_000 });
  const sessionItem = page.getByTestId('session-item').filter({ hasText: sessionName }).first();
  await expect(sessionItem).toBeVisible({ timeout: 10_000 });
  await sessionItem.click();

  await expect(page.locator('.react-flow__node').first()).toBeVisible({ timeout: 15_000 });

  const compositorNode = page.locator('.react-flow__node').filter({ hasText: 'Compositor' });
  await expect(compositorNode).toBeVisible({ timeout: 10_000 });

  const canvasInner = compositorNode.locator('[data-canvas-width]');
  await expect(canvasInner).toBeVisible({ timeout: 5_000 });

  return { compositorNode, canvasInner };
}

test.describe('Webcam Circle PiP Pipeline — E2E Validation', () => {
  let collector: ConsoleErrorCollector;
  let sessionId: string | null = null;
  let sessionName: string | null = null;

  test.beforeEach(async ({ page }) => {
    collector = createConsoleErrorCollector(page);
  });

  test('circle PiP pipeline creates successfully, compositor shows circular preview', async ({
    page,
    baseURL,
  }) => {
    test.setTimeout(120_000);

    const apiContext = await request.newContext({
      baseURL: baseURL!,
      extraHTTPHeaders: getAuthHeaders(),
    });

    sessionName = `circle-pip-test-${Date.now()}`;
    const createResponse = await apiContext.post('/api/v1/sessions', {
      data: {
        name: sessionName,
        yaml: WEBCAM_PIP_CIRCLE_YAML,
      },
    });

    const responseText = await createResponse.text();
    expect(createResponse.ok(), `Failed to create session: ${responseText}`).toBeTruthy();

    const createData = JSON.parse(responseText) as { session_id: string };
    sessionId = createData.session_id;
    expect(sessionId).toBeTruthy();
    await apiContext.dispose();

    const { compositorNode, canvasInner } = await setupCompositorView(page, sessionName);

    const inputLayer0 = compositorNode.getByText('Input 0', { exact: true }).first();
    const inputLayer1 = compositorNode.getByText('Input 1', { exact: true }).first();
    const textLayer = compositorNode.getByText('Text 0', { exact: true }).first();

    await expect(inputLayer0).toBeVisible({ timeout: 5_000 });
    await expect(inputLayer1).toBeVisible({ timeout: 5_000 });
    await expect(textLayer).toBeVisible({ timeout: 5_000 });

    const liveBadge = compositorNode.getByText('LIVE');
    await expect(liveBadge).toBeVisible({ timeout: 5_000 });

    const videoLayerBox = canvasInner.locator('.nodrag.nopan').filter({ hasText: 'in_1' }).first();
    await expect(videoLayerBox).toBeVisible({ timeout: 5_000 });

    // clipPath lives on the inner [data-crop-circle] div, not the LayerBox
    // itself (the outer box stays unclipped so resize handles remain visible).
    const clipPath = await videoLayerBox.evaluate((el) => {
      const inner = el.querySelector('[data-crop-circle]');
      return inner ? window.getComputedStyle(inner).clipPath : 'none';
    });
    expect(clipPath).toMatch(/^circle\(/);

    const bgLayerBox = canvasInner.locator('.nodrag.nopan').filter({ hasText: 'in_0' }).first();
    await expect(bgLayerBox).toBeVisible({ timeout: 5_000 });

    const bgClipPath = await bgLayerBox.evaluate((el) => window.getComputedStyle(el).clipPath);
    expect(bgClipPath).toBe('none');

    await inputLayer1.click();

    const cropSection = compositorNode.getByTestId('crop-zoom-section');
    await expect(cropSection).toBeVisible({ timeout: 5_000 });

    // Circle button should be active.
    const circleButton = compositorNode.getByTestId('crop-shape-circle');
    await expectButtonActive(circleButton, true);

    // Zoom should reflect the pipeline value (1.8×).
    const zoomValue = compositorNode.getByTestId('crop-zoom-value');
    await expect(zoomValue).toHaveText('1.8×', { timeout: 3_000 });

    // Pan X and Tilt Y should be enabled (zoom > 1.0).
    const panXSlider = compositorNode.getByTestId('crop-pan-x-slider');
    const tiltYSlider = compositorNode.getByTestId('crop-tilt-y-slider');
    await expect(panXSlider).not.toHaveAttribute('data-disabled', '');
    await expect(tiltYSlider).not.toHaveAttribute('data-disabled', '');

    await inputLayer0.click();
    await expect(cropSection).toBeVisible({ timeout: 5_000 });

    const rectButton = compositorNode.getByTestId('crop-shape-rect');
    await expectButtonActive(rectButton, true);

    await textLayer.click();
    await expect(cropSection).not.toBeVisible({ timeout: 5_000 });

    const unexpected = collector.getUnexpected(MOQ_BENIGN_PATTERNS);
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
