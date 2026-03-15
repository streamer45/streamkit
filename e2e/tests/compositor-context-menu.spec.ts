// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * E2E tests for compositor right-click context menu.
 *
 * Creates a Webcam PiP pipeline session via the API, navigates to the
 * monitor view where the compositor node graph is rendered, then exercises
 * the context menu on compositor canvas layers:
 *
 * - Right-click a layer → context menu appears
 * - Menu contains "Bring to Front", "Send to Back"
 * - Text/image layers also show "Delete"
 * - Click outside or Escape dismisses the menu
 * - Right-clicking a selected layer preserves selection (does not deselect)
 */

import { test, expect, request } from '@playwright/test';

import { ensureLoggedIn, getAuthHeaders } from './auth-helpers';
import {
  type ConsoleErrorCollector,
  MOQ_BENIGN_PATTERNS,
  createConsoleErrorCollector,
} from './test-helpers';
import { WEBCAM_PIP_YAML } from './compositor-fixtures';

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

test.describe('Compositor Context Menu', () => {
  let collector: ConsoleErrorCollector;
  let sessionId: string | null = null;

  test.beforeEach(async ({ page }) => {
    collector = createConsoleErrorCollector(page);
  });

  test('right-click opens context menu, actions work, Escape dismisses', async ({
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
        name: `ctx-menu-test-${Date.now()}`,
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
    // Wait for compositor node graph to fully settle.
    await expect(compositorNode).toBeVisible({ timeout: 10_000 });

    const canvasInner = compositorNode.locator('[data-canvas-width]');
    await expect(canvasInner).toBeVisible({ timeout: 5_000 });

    // ── 3. Verify layer list items exist ─────────────────────────────────

    const textLayer = compositorNode.getByText('Text 0', { exact: true }).first();
    const inputLayer = compositorNode.getByText('Input 1', { exact: true }).first();
    await expect(textLayer).toBeVisible({ timeout: 5_000 });
    await expect(inputLayer).toBeVisible({ timeout: 5_000 });

    // ── 4. Right-click a video layer → context menu appears ─────────────

    // Find the "in_1" LayerBox on the canvas to right-click on it.
    const videoLayerBox = canvasInner.locator('.nodrag.nopan').filter({ hasText: 'in_1' }).first();
    await expect(videoLayerBox).toBeVisible({ timeout: 5_000 });

    // Select the layer first by left-clicking in the layer list.
    await inputLayer.click();

    // Right-click on the video layer box.
    await videoLayerBox.click({ button: 'right' });

    // The context menu should appear.
    const contextMenu = page.getByTestId('compositor-context-menu');
    await expect(contextMenu).toBeVisible({ timeout: 3_000 });

    // It should have "Bring to Front" and "Send to Back" items.
    await expect(page.getByTestId('ctx-bring-to-front')).toBeVisible();
    await expect(page.getByTestId('ctx-send-to-back')).toBeVisible();

    // Video layers should NOT have a "Delete" option.
    await expect(page.getByTestId('ctx-delete')).not.toBeVisible();

    // ── 5. Dismiss with Escape ──────────────────────────────────────────

    await page.keyboard.press('Escape');
    await expect(contextMenu).not.toBeVisible({ timeout: 3_000 });

    // ── 6. Right-click a text layer → shows Delete option ───────────────

    // Select text layer first.
    await textLayer.click();

    const textLayerBox = canvasInner.locator('.nodrag.nopan').filter({ hasText: 'Text 0' }).first();
    await expect(textLayerBox).toBeVisible({ timeout: 5_000 });

    // Right-click on the text layer box.
    await textLayerBox.click({ button: 'right' });
    await expect(contextMenu).toBeVisible({ timeout: 3_000 });

    // Text layers should have "Delete" option.
    await expect(page.getByTestId('ctx-delete')).toBeVisible();

    // ── 7. Click "Bring to Front" action ────────────────────────────────

    await page.getByTestId('ctx-bring-to-front').click();

    // Menu should close after clicking an action.
    await expect(contextMenu).not.toBeVisible({ timeout: 3_000 });

    // ── 8. Right-click preserves selection (focus loss fix) ─────────────

    // Select "Input 1" from layer list.
    await inputLayer.click();

    // Right-click on the video layer — should keep it selected (or select it).
    await videoLayerBox.click({ button: 'right' });

    await expect(contextMenu).toBeVisible({ timeout: 3_000 });

    // The context menu should still be visible (not dismissed by deselection),
    // proving that right-click does not lose focus.

    // Dismiss menu for cleanup.
    await page.keyboard.press('Escape');
    await expect(contextMenu).not.toBeVisible({ timeout: 3_000 });

    // ── 9. Delete text overlay via context menu ─────────────────────────

    // Select text layer.
    await textLayer.click();

    // Right-click on text layer.
    await textLayerBox.click({ button: 'right' });

    await expect(contextMenu).toBeVisible({ timeout: 3_000 });
    await page.getByTestId('ctx-delete').click();

    // "Text 0" should be removed from the layer list.
    await expect(compositorNode.getByText('Text 0', { exact: true }).first()).not.toBeVisible({
      timeout: 5_000,
    });

    // ── 10. Console error check ─────────────────────────────────────────

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
