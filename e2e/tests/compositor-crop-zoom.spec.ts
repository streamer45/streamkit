// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * E2E tests for compositor crop/pan/tilt/zoom controls.
 *
 * Creates a Webcam PiP pipeline session via the API (with crop_zoom on
 * the PiP layer), navigates to the monitor view, then exercises:
 *
 * - Crop & Zoom section appears for video layers
 * - Zoom slider reflects the initial crop_zoom value from the pipeline
 * - Pan X / Tilt Y sliders are enabled when zoom > 1.0
 * - Reset button restores defaults (zoom=1.0, panX=0.5, tiltY=0.5)
 * - Crop & Zoom section is NOT shown for text overlays
 */

import { test, expect, request, type Page } from '@playwright/test';

import { ensureLoggedIn, getAuthHeaders } from './auth-helpers';
import {
  type ConsoleErrorCollector,
  MOQ_BENIGN_PATTERNS,
  createConsoleErrorCollector,
} from './test-helpers';
import { WEBCAM_PIP_CROPPED_YAML } from './compositor-fixtures';

/**
 * Navigate to the monitor view, open the first session, wait for the
 * compositor node to render, and return the compositor node locator
 * together with the canvas inner locator.
 */
async function setupCompositorView(page: Page) {
  await page.goto('/monitor');
  await ensureLoggedIn(page);
  if (!page.url().includes('/monitor')) {
    await page.goto('/monitor');
  }
  await expect(page.getByTestId('monitor-view')).toBeVisible({ timeout: 15_000 });

  // Wait for sessions list and click the session.
  await expect(page.getByTestId('sessions-list')).toBeVisible({ timeout: 10_000 });
  const sessionItem = page.getByTestId('session-item').first();
  await expect(sessionItem).toBeVisible({ timeout: 10_000 });
  await sessionItem.click();

  // Wait for the React Flow canvas and compositor node.
  await expect(page.locator('.react-flow__node').first()).toBeVisible({ timeout: 15_000 });

  const compositorNode = page.locator('.react-flow__node').filter({ hasText: 'Compositor' });
  await expect(compositorNode).toBeVisible({ timeout: 10_000 });

  const canvasInner = compositorNode.locator('[data-canvas-width]');
  await expect(canvasInner).toBeVisible({ timeout: 5_000 });

  return { compositorNode, canvasInner };
}

test.describe('Compositor Crop & Zoom Controls', () => {
  let collector: ConsoleErrorCollector;
  let sessionId: string | null = null;

  test.beforeEach(async ({ page }) => {
    collector = createConsoleErrorCollector(page);
  });

  test('crop/zoom controls appear for video layers, not for text overlays', async ({
    page,
    baseURL,
  }) => {
    test.setTimeout(120_000);

    const apiContext = await request.newContext({
      baseURL: baseURL!,
      extraHTTPHeaders: getAuthHeaders(),
    });

    const createResponse = await apiContext.post('/api/v1/sessions', {
      data: {
        name: `crop-zoom-test-${Date.now()}`,
        yaml: WEBCAM_PIP_CROPPED_YAML,
      },
    });

    const responseText = await createResponse.text();
    expect(createResponse.ok(), `Failed to create session: ${responseText}`).toBeTruthy();

    const createData = JSON.parse(responseText) as { session_id: string };
    sessionId = createData.session_id;
    expect(sessionId).toBeTruthy();
    await apiContext.dispose();

    const { compositorNode } = await setupCompositorView(page);

    // Verify expected layers are visible in the layer list.
    const inputLayer0 = compositorNode.getByText('Input 0', { exact: true }).first();
    const inputLayer1 = compositorNode.getByText('Input 1', { exact: true }).first();
    const textLayer = compositorNode.getByText('Text 0', { exact: true }).first();
    await expect(inputLayer0).toBeVisible({ timeout: 5_000 });
    await expect(inputLayer1).toBeVisible({ timeout: 5_000 });
    await expect(textLayer).toBeVisible({ timeout: 5_000 });

    await inputLayer1.click();

    const cropSection = compositorNode.getByTestId('crop-zoom-section');
    await expect(cropSection).toBeVisible({ timeout: 5_000 });

    // Verify the zoom slider and value display are visible.
    const zoomSlider = compositorNode.getByTestId('crop-zoom-slider');
    const zoomValue = compositorNode.getByTestId('crop-zoom-value');
    await expect(zoomSlider).toBeVisible();
    await expect(zoomValue).toBeVisible();

    // The initial zoom should be 2.0× (from the pipeline YAML).
    await expect(zoomValue).toHaveText('2.0×');

    // Pan X and Tilt Y sliders should be visible and enabled (zoom > 1.0).
    const panXSlider = compositorNode.getByTestId('crop-pan-x-slider');
    const tiltYSlider = compositorNode.getByTestId('crop-tilt-y-slider');
    await expect(panXSlider).toBeVisible();
    await expect(tiltYSlider).toBeVisible();
    // Radix slider uses aria-disabled when disabled; verify NOT disabled.
    await expect(panXSlider).not.toHaveAttribute('data-disabled', '');
    await expect(tiltYSlider).not.toHaveAttribute('data-disabled', '');

    const resetButton = compositorNode.getByTestId('crop-zoom-reset');
    await expect(resetButton).toBeVisible();
    await resetButton.click();

    // After reset: zoom=1.0, pan/tilt should be disabled.
    await expect(zoomValue).toHaveText('1.0×', { timeout: 3_000 });

    // Pan/tilt sliders should now be disabled (zoom <= 1.0).
    await expect(panXSlider).toHaveAttribute('data-disabled', '', { timeout: 3_000 });
    await expect(tiltYSlider).toHaveAttribute('data-disabled', '', { timeout: 3_000 });

    await inputLayer0.click();
    await expect(cropSection).toBeVisible({ timeout: 5_000 });

    // in_0 has default crop (zoom=1.0), so pan/tilt should be disabled.
    await expect(zoomValue).toHaveText('1.0×', { timeout: 3_000 });

    await textLayer.click();

    // The crop section should disappear for text overlays.
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
