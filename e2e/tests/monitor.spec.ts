// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { test, expect, request } from '@playwright/test';

import { ensureLoggedIn, getAuthHeaders } from './auth-helpers';

test.describe('Monitor View - Session Lifecycle', () => {
  // Unique session name for this test run
  const testSessionName = `e2e-test-session-${Date.now()}`;
  let sessionId: string | null = null;

  // Minimal dynamic session YAML that doesn't require MoQ/plugins
  // Uses core::file_reader (source with no inputs) → core::sink
  const minimalPipelineYaml = `mode: dynamic
steps:
  - kind: core::file_reader
    params:
      path: samples/audio/system/ehren-paper_lights-96.opus
  - kind: core::sink
`;

  test.beforeEach(async ({ page }) => {
    await page.goto('/monitor');
    await ensureLoggedIn(page);
    if (!page.url().includes('/monitor')) {
      await page.goto('/monitor');
    }
    await expect(page.getByTestId('monitor-view')).toBeVisible();
  });

  test('creates session via API, verifies it appears in UI, then deletes it', async ({
    page,
    baseURL,
  }) => {
    const apiContext = await request.newContext({
      baseURL: baseURL!,
      extraHTTPHeaders: getAuthHeaders(),
    });

    try {
      // Step 1: Create session via API
      const createResponse = await apiContext.post('/api/v1/sessions', {
        data: {
          name: testSessionName,
          yaml: minimalPipelineYaml,
        },
      });
      const responseText = await createResponse.text();
      expect(createResponse.ok(), `Create session failed: ${responseText}`).toBeTruthy();
      const createData = JSON.parse(responseText) as { session_id: string };
      sessionId = createData.session_id;
      expect(sessionId).toBeTruthy();

      // Step 2: Refresh the page to see the new session
      await page.reload();
      await expect(page.getByTestId('monitor-view')).toBeVisible();

      // Wait for sessions list to load
      await expect(page.getByTestId('sessions-list')).toBeVisible({ timeout: 10000 });

      // Find our session in the list by name
      const sessionItem = page.getByTestId('session-item').filter({ hasText: testSessionName });
      await expect(sessionItem).toBeVisible({ timeout: 10000 });

      // Step 3: Delete the session via UI
      await sessionItem.hover();
      const deleteButton = sessionItem.getByTestId('session-delete-btn');
      await expect(deleteButton).toBeVisible();
      await deleteButton.click();

      // Confirm deletion in modal (scope to modal)
      const confirmModal = page.getByTestId('confirm-modal');
      await expect(confirmModal).toBeVisible();
      await confirmModal.getByRole('button', { name: /Confirm|Delete/i }).click();

      // Verify session is removed from the list
      await expect(sessionItem).toHaveCount(0, { timeout: 10000 });

      // Mark as cleaned up so afterEach doesn't try to delete again
      sessionId = null;
    } finally {
      await apiContext.dispose();
    }
  });

  test('deleted session does not reappear in the list (race condition regression)', async ({
    page,
    baseURL,
  }) => {
    const apiContext = await request.newContext({
      baseURL: baseURL!,
      extraHTTPHeaders: getAuthHeaders(),
    });

    try {
      // Step 1: Create a session via the API.
      const createResponse = await apiContext.post('/api/v1/sessions', {
        data: {
          name: testSessionName,
          yaml: minimalPipelineYaml,
        },
      });
      const responseText = await createResponse.text();
      expect(createResponse.ok(), `Create session failed: ${responseText}`).toBeTruthy();
      const createData = JSON.parse(responseText) as { session_id: string };
      sessionId = createData.session_id;
      expect(sessionId).toBeTruthy();

      // Step 2: Reload and wait for the session to appear.
      await page.reload();
      await expect(page.getByTestId('monitor-view')).toBeVisible();
      await expect(page.getByTestId('sessions-list')).toBeVisible({ timeout: 10000 });

      const sessionItem = page.getByTestId('session-item').filter({ hasText: testSessionName });
      await expect(sessionItem).toBeVisible({ timeout: 10000 });

      // Step 3: Delete the session via the UI.
      await sessionItem.hover();
      const deleteButton = sessionItem.getByTestId('session-delete-btn');
      await expect(deleteButton).toBeVisible();
      await deleteButton.click();

      const confirmModal = page.getByTestId('confirm-modal');
      await expect(confirmModal).toBeVisible();
      await confirmModal.getByRole('button', { name: /Confirm|Delete/i }).click();

      // Step 4: Verify the session disappears.
      await expect(sessionItem).toHaveCount(0, { timeout: 10000 });

      // Step 5: Wait several seconds and assert the session does NOT reappear.
      // The old code would re-fetch the session list immediately after the
      // WebSocket event, and the server could still return the stale session
      // causing a brief flicker.  With the fix the optimistic removal prevents
      // any reappearance.
      await page.waitForTimeout(5000);
      await expect(sessionItem).toHaveCount(0);

      sessionId = null;
    } finally {
      await apiContext.dispose();
    }
  });

  test.afterEach(async ({ baseURL }) => {
    // Cleanup: ensure session is deleted even if test fails
    if (sessionId) {
      try {
        const apiContext = await request.newContext({
          baseURL: baseURL!,
          extraHTTPHeaders: getAuthHeaders(),
        });
        await apiContext.delete(`/api/v1/sessions/${sessionId}`);
        await apiContext.dispose();
      } catch {
        // Ignore cleanup errors
      }
      sessionId = null;
    }
  });
});
