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
import { WEBCAM_PIP_YAML } from './compositor-fixtures';

/** SNAP_GRID from compositorLayerParsers — default arrow-key nudge step. */
const SNAP_GRID = 10;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/**
 * Focus the compositor keyboard wrapper and dispatch a keydown event.
 *
 * Both focus and key dispatch happen inside a single page.evaluate() call
 * so there is no async gap where focus could be lost between the two
 * operations.  The event is dispatched directly on the wrapper element,
 * which is where the compositorKeyboard hook's listener is attached.
 */
async function pressKey(page: Page, combo: string) {
  await page.evaluate((c) => {
    const el =
      document.querySelector('[data-testid="compositor-keyboard-target"]') ??
      document.querySelector('.react-flow__node [tabindex="-1"]');
    if (!(el instanceof HTMLElement)) return;

    el.focus();

    // Parse combo like "Shift+ArrowDown" → { key: "ArrowDown", shiftKey: true }
    const parts = c.split('+');
    const key = parts[parts.length - 1];
    const shiftKey = parts.includes('Shift');

    el.dispatchEvent(
      new KeyboardEvent('keydown', { key, shiftKey, bubbles: true, cancelable: true })
    );
  }, combo);
  await page.waitForTimeout(300);
}

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

  const compositorNode = page.locator('.react-flow__node').filter({ hasText: 'Compositor' });
  await expect(compositorNode).toBeVisible({ timeout: 10_000 });

  const canvasInner = compositorNode.locator('[data-canvas-width]');
  await expect(canvasInner).toBeVisible({ timeout: 5_000 });

  return { compositorNode, canvasInner };
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

    const { compositorNode, canvasInner } = await setupCompositorView(page);

    // Verify expected layers are visible in the layer list.
    const inputLayer = compositorNode.getByText('Input 1', { exact: true }).first();
    const textLayer = compositorNode.getByText('Text 0', { exact: true }).first();
    await expect(inputLayer).toBeVisible({ timeout: 5_000 });
    await expect(textLayer).toBeVisible({ timeout: 5_000 });

    // ── 3. Arrow key nudge on a video layer ─────────────────────────────

    // Select "Input 1" in the layer list.
    await inputLayer.click();

    // Read the initial position of the "in_1" LayerBox on the canvas.
    const pos0 = await getCanvasLayerPosition(canvasInner, 'in_1');
    expect(pos0, 'Video layer "in_1" not found on canvas').not.toBeNull();

    // ArrowRight → nudge right by SNAP_GRID.
    await pressKey(page, 'ArrowRight');

    const pos1 = await getCanvasLayerPosition(canvasInner, 'in_1');
    expect(pos1).not.toBeNull();
    expect(pos1!.left).toBe(pos0!.left + SNAP_GRID);
    expect(pos1!.top).toBe(pos0!.top);

    // Shift+ArrowDown → fine nudge down by 1 px.
    await pressKey(page, 'Shift+ArrowDown');

    const pos2 = await getCanvasLayerPosition(canvasInner, 'in_1');
    expect(pos2).not.toBeNull();
    expect(pos2!.left).toBe(pos1!.left);
    expect(pos2!.top).toBe(pos1!.top + 1);

    // ── 4. Delete on a video layer is a no-op ───────────────────────────

    await pressKey(page, 'Delete');

    // The video layer must still exist.
    const posAfterDelete = await getCanvasLayerPosition(canvasInner, 'in_1');
    expect(posAfterDelete, 'Video layer should survive Delete').not.toBeNull();
    expect(posAfterDelete!.left).toBe(pos2!.left);

    // ── 5. Escape deselects the current layer ───────────────────────────

    await pressKey(page, 'Escape');

    // After Escape, arrow keys should be a no-op (nothing selected).
    await pressKey(page, 'ArrowRight');

    const posAfterEsc = await getCanvasLayerPosition(canvasInner, 'in_1');
    expect(posAfterEsc).not.toBeNull();
    expect(posAfterEsc!.left).toBe(pos2!.left); // unchanged

    // ── 6. Arrow key nudge on a text overlay ────────────────────────────

    await textLayer.click();

    const textPos0 = await getCanvasLayerPosition(canvasInner, 'Text 0');
    expect(textPos0, 'Text overlay "Text 0" not found on canvas').not.toBeNull();

    await pressKey(page, 'ArrowLeft');

    const textPos1 = await getCanvasLayerPosition(canvasInner, 'Text 0');
    expect(textPos1).not.toBeNull();
    expect(textPos1!.left).toBe(textPos0!.left - SNAP_GRID);
    expect(textPos1!.top).toBe(textPos0!.top);

    // ── 7. Delete removes the text overlay ──────────────────────────────

    await pressKey(page, 'Delete');

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
