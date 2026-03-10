// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * E2E tests for compositor keyboard shortcuts.
 *
 * Creates a Webcam PiP pipeline session via the API, navigates to the
 * monitor view where the full compositor node graph is rendered, then
 * exercises the keyboard shortcuts added in compositorKeyboard.ts:
 *
 * - Arrow keys → nudge selected layer by SNAP_GRID (10 px)
 * - Shift+Arrow → fine nudge by 1 px
 * - Escape → deselect the current layer
 * - Delete → remove a text overlay (video layers silently ignored)
 */

import { test, expect, request, type Page, type Locator } from '@playwright/test';

import { ensureLoggedIn, getAuthHeaders } from './auth-helpers';
import {
  type ConsoleErrorCollector,
  MOQ_BENIGN_PATTERNS,
  createConsoleErrorCollector,
} from './test-helpers';

// ---------------------------------------------------------------------------
// Pipeline YAML — Webcam PiP compositor (same as compositor-perf.spec.ts)
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

/** SNAP_GRID from compositorLayerParsers — default arrow-key nudge step. */
const SNAP_GRID = 10;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/**
 * Read the inline `left` / `top` pixel values from a LayerBox inside the
 * compositor canvas, identified by its label text (e.g. "in_1", "Text 0").
 *
 * Returns `null` if the element cannot be found.
 */
async function getCanvasLayerPosition(
  canvasInner: Locator,
  labelText: string
): Promise<{ left: number; top: number } | null> {
  const layerBox = canvasInner.locator('.nodrag.nopan').filter({ hasText: labelText }).first();
  if (!(await layerBox.isVisible().catch(() => false))) return null;

  return layerBox.evaluate((el) => ({
    left: parseFloat((el as HTMLElement).style.left),
    top: parseFloat((el as HTMLElement).style.top),
  }));
}

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
  await page.waitForTimeout(2_000);

  const compositorNode = page.locator('.react-flow__node').filter({ hasText: 'Compositor' });
  await expect(compositorNode).toBeVisible({ timeout: 10_000 });

  const canvasInner = compositorNode.locator('[data-canvas-width]');
  await expect(canvasInner).toBeVisible({ timeout: 5_000 });

  // The compositor wrapper (tabIndex={-1}) receives keyboard events.
  // Using locator.press() on this element is more reliable than
  // page.keyboard.press() because it explicitly focuses the element
  // before dispatching — avoiding races with React's useEffect focus.
  const wrapper = compositorNode.locator('[tabindex="-1"]').first();

  return { compositorNode, canvasInner, wrapper };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

test.describe('Compositor Keyboard Shortcuts', () => {
  let collector: ConsoleErrorCollector;
  let sessionId: string | null = null;

  test.beforeEach(async ({ page }) => {
    collector = createConsoleErrorCollector(page);
  });

  test('arrow keys nudge layers, Escape deselects, Delete removes text overlay', async ({
    page,
    baseURL,
  }) => {
    test.setTimeout(120_000);

    // ── 1. Create session via API ────────────────────────────────────────

    const apiContext = await request.newContext({
      baseURL: baseURL!,
      extraHTTPHeaders: getAuthHeaders(),
    });

    const createResponse = await apiContext.post('/api/v1/sessions', {
      data: {
        name: `kbd-test-${Date.now()}`,
        yaml: WEBCAM_PIP_YAML,
      },
    });

    const responseText = await createResponse.text();
    expect(createResponse.ok(), `Failed to create session: ${responseText}`).toBeTruthy();

    const createData = JSON.parse(responseText) as { session_id: string };
    sessionId = createData.session_id;
    expect(sessionId).toBeTruthy();
    await apiContext.dispose();

    // ── 2. Navigate to monitor view, find compositor node ────────────────

    const { compositorNode, canvasInner, wrapper } = await setupCompositorView(page);

    // Verify expected layers are visible in the layer list.
    const inputLayer = compositorNode.getByText('Input 1', { exact: true }).first();
    const textLayer = compositorNode.getByText('Text 0', { exact: true }).first();
    await expect(inputLayer).toBeVisible({ timeout: 5_000 });
    await expect(textLayer).toBeVisible({ timeout: 5_000 });

    // ── 3. Arrow key nudge on a video layer ─────────────────────────────

    // Select "Input 1" in the layer list.
    await inputLayer.click();
    await page.waitForTimeout(500);

    // Read the initial position of the "in_1" LayerBox on the canvas.
    const pos0 = await getCanvasLayerPosition(canvasInner, 'in_1');
    expect(pos0, 'Video layer "in_1" not found on canvas').not.toBeNull();

    // ArrowRight → nudge right by SNAP_GRID.
    await wrapper.press('ArrowRight');
    await page.waitForTimeout(300);

    const pos1 = await getCanvasLayerPosition(canvasInner, 'in_1');
    expect(pos1).not.toBeNull();
    expect(pos1!.left).toBe(pos0!.left + SNAP_GRID);
    expect(pos1!.top).toBe(pos0!.top);

    // Shift+ArrowDown → fine nudge down by 1 px.
    await wrapper.press('Shift+ArrowDown');
    await page.waitForTimeout(300);

    const pos2 = await getCanvasLayerPosition(canvasInner, 'in_1');
    expect(pos2).not.toBeNull();
    expect(pos2!.left).toBe(pos1!.left);
    expect(pos2!.top).toBe(pos1!.top + 1);

    // ── 4. Delete on a video layer is a no-op ───────────────────────────

    await wrapper.press('Delete');
    await page.waitForTimeout(300);

    // The video layer must still exist.
    const posAfterDelete = await getCanvasLayerPosition(canvasInner, 'in_1');
    expect(posAfterDelete, 'Video layer should survive Delete').not.toBeNull();
    expect(posAfterDelete!.left).toBe(pos2!.left);

    // ── 5. Escape deselects the current layer ───────────────────────────

    await wrapper.press('Escape');
    await page.waitForTimeout(300);

    // After Escape, arrow keys should be a no-op (nothing selected).
    await wrapper.press('ArrowRight');
    await page.waitForTimeout(300);

    const posAfterEsc = await getCanvasLayerPosition(canvasInner, 'in_1');
    expect(posAfterEsc).not.toBeNull();
    expect(posAfterEsc!.left).toBe(pos2!.left); // unchanged

    // ── 6. Arrow key nudge on a text overlay ────────────────────────────

    await textLayer.click();
    await page.waitForTimeout(500);

    const textPos0 = await getCanvasLayerPosition(canvasInner, 'Text 0');
    expect(textPos0, 'Text overlay "Text 0" not found on canvas').not.toBeNull();

    await wrapper.press('ArrowLeft');
    await page.waitForTimeout(300);

    const textPos1 = await getCanvasLayerPosition(canvasInner, 'Text 0');
    expect(textPos1).not.toBeNull();
    expect(textPos1!.left).toBe(textPos0!.left - SNAP_GRID);
    expect(textPos1!.top).toBe(textPos0!.top);

    // ── 7. Delete removes the text overlay ──────────────────────────────

    await wrapper.press('Delete');
    await page.waitForTimeout(500);

    // "Text 0" should no longer appear in the layer list.
    await expect(compositorNode.getByText('Text 0', { exact: true }).first()).not.toBeVisible({
      timeout: 5_000,
    });

    // ── 8. Console error check ──────────────────────────────────────────

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
