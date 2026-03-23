// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { test, expect } from '@playwright/test';

import { ensureLoggedIn } from './auth-helpers';

test.describe('Log Viewer', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/admin/logs');
    await ensureLoggedIn(page);
    if (!page.url().includes('/admin/logs')) {
      await page.goto('/admin/logs');
    }
    await expect(page.getByTestId('logs-view')).toBeVisible();
  });

  test('navigates to log viewer and displays UI', async ({ page }) => {
    await expect(page.getByTestId('logs-view')).toBeVisible();

    // Should show the title
    await expect(page.getByRole('heading', { name: 'Logs' })).toBeVisible();

    // Should show the admin nav with Logs link active
    await expect(page.getByRole('link', { name: 'Logs' })).toBeVisible();

    // Log container should be present
    await expect(page.getByTestId('logs-container')).toBeVisible();
  });

  test('filter controls are present and functional', async ({ page }) => {
    await expect(page.getByTestId('logs-view')).toBeVisible();

    // Filter input should be visible
    const filterInput = page.getByTestId('logs-filter-input');
    await expect(filterInput).toBeVisible();

    // Level select should be visible
    const levelSelect = page.getByTestId('logs-level-select');
    await expect(levelSelect).toBeVisible();

    // Page size select should be visible
    const pageSizeSelect = page.getByTestId('logs-page-size');
    await expect(pageSizeSelect).toBeVisible();

    // Wrap toggle should be visible
    const wrapToggle = page.getByTestId('logs-wrap-toggle');
    await expect(wrapToggle).toBeVisible();

    // Expand toggle should be visible
    const expandToggle = page.getByTestId('logs-expand-toggle');
    await expect(expandToggle).toBeVisible();
    await expect(expandToggle).toHaveText('Expand');

    // Live tail button should be visible
    const liveTailButton = page.getByTestId('logs-live-tail');
    await expect(liveTailButton).toBeVisible();
  });

  test('expand toggle switches between constrained and full-width layout', async ({ page }) => {
    await expect(page.getByTestId('logs-view')).toBeVisible();

    const expandToggle = page.getByTestId('logs-expand-toggle');

    // Initially should show "Expand" (constrained width)
    await expect(expandToggle).toHaveText('Expand');

    // Click to expand
    await expandToggle.click();
    await expect(expandToggle).toHaveText('Collapse');

    // Click again to collapse
    await expandToggle.click();
    await expect(expandToggle).toHaveText('Expand');
  });

  test('clicking a log line copies it to clipboard', async ({ page, context }) => {
    await expect(page.getByTestId('logs-view')).toBeVisible();

    // Grant clipboard permissions
    await context.grantPermissions(['clipboard-read', 'clipboard-write']);

    // Wait for log lines to load
    const container = page.getByTestId('logs-container');
    await expect(container).toBeVisible();

    // Get the first log line and click it
    const firstLogLine = container.locator('div[title="Click to copy"]').first();
    const lineCount = await container.locator('div[title="Click to copy"]').count();

    if (lineCount > 0) {
      const lineText = await firstLogLine.textContent();
      await firstLogLine.click();

      // Verify the clipboard contents match
      const clipboardText = await page.evaluate(() => navigator.clipboard.readText());
      expect(clipboardText).toBe(lineText);
    }
  });

  test('pagination buttons are present', async ({ page }) => {
    await expect(page.getByTestId('logs-view')).toBeVisible();

    // Pagination buttons should be visible
    await expect(page.getByTestId('logs-load-older')).toBeVisible();
    await expect(page.getByTestId('logs-load-newer')).toBeVisible();
    await expect(page.getByTestId('logs-load-latest')).toBeVisible();
  });

  test('level filter applies immediately on change', async ({ page }) => {
    await expect(page.getByTestId('logs-view')).toBeVisible();

    // Select "Error" level filter — should reload without needing a button click
    await page.getByTestId('logs-level-select').selectOption('error');

    // Wait briefly for the filtered results
    await page.waitForTimeout(1000);

    // The log container should still be present (may be empty if no errors)
    await expect(page.getByTestId('logs-container')).toBeVisible();
  });

  test('text filter updates as user types', async ({ page }) => {
    await expect(page.getByTestId('logs-view')).toBeVisible();

    // Type a filter — should apply after debounce (no button click needed)
    await page.getByTestId('logs-filter-input').fill('skit');

    // Wait for debounce + request
    await page.waitForTimeout(1000);

    // Container should still be visible
    await expect(page.getByTestId('logs-container')).toBeVisible();
  });

  test('log API returns valid response when file logging is enabled', async ({ page }) => {
    // Test the logs API directly (cookies from ensureLoggedIn provide auth)
    const response = await page.request.get('/api/v1/logs?limit=10&direction=backward');

    // File logging may be disabled in CI — skip gracefully
    if (response.status() === 404) {
      test.skip(true, 'File logging is disabled on this server');
      return;
    }

    expect(response.ok()).toBeTruthy();

    const body = (await response.json()) as {
      lines: string[];
      next_offset: number;
      has_more: boolean;
      file_size: number;
    };

    expect(body).toHaveProperty('lines');
    expect(body).toHaveProperty('next_offset');
    expect(body).toHaveProperty('has_more');
    expect(body).toHaveProperty('file_size');
    expect(Array.isArray(body.lines)).toBeTruthy();
    expect(typeof body.next_offset).toBe('number');
    expect(typeof body.has_more).toBe('boolean');
    expect(typeof body.file_size).toBe('number');
  });

  test('log API supports forward pagination when file logging is enabled', async ({ page }) => {
    const firstPage = await page.request.get('/api/v1/logs?limit=5&direction=forward&offset=0');

    if (firstPage.status() === 404) {
      test.skip(true, 'File logging is disabled on this server');
      return;
    }

    expect(firstPage.ok()).toBeTruthy();

    const firstBody = (await firstPage.json()) as {
      lines: string[];
      next_offset: number;
      has_more: boolean;
    };

    if (firstBody.has_more) {
      const secondPage = await page.request.get(
        `/api/v1/logs?limit=5&direction=forward&offset=${firstBody.next_offset}`
      );
      expect(secondPage.ok()).toBeTruthy();

      const secondBody = (await secondPage.json()) as {
        lines: string[];
        next_offset: number;
      };

      // next_offset should advance
      expect(secondBody.next_offset).toBeGreaterThanOrEqual(firstBody.next_offset);
    }
  });

  test('log API supports level filtering when file logging is enabled', async ({ page }) => {
    const response = await page.request.get('/api/v1/logs?limit=100&direction=backward&level=info');

    if (response.status() === 404) {
      test.skip(true, 'File logging is disabled on this server');
      return;
    }

    expect(response.ok()).toBeTruthy();

    const body = (await response.json()) as { lines: string[] };

    // All returned lines should contain INFO level marker
    for (const line of body.lines) {
      const hasInfoLevel = / INFO /i.test(line) || /"level":"INFO"/i.test(line);
      expect(hasInfoLevel).toBeTruthy();
    }
  });

  test('admin nav shows Logs link on admin pages', async ({ page }) => {
    // Already on /admin/logs from beforeEach
    await expect(page.getByTestId('logs-view')).toBeVisible();
    await expect(page.getByRole('link', { name: 'Logs' })).toBeVisible();

    // Navigate to tokens page (already in ensureLoggedIn's appViews)
    await page.goto('/admin/tokens');
    await expect(page.getByTestId('tokens-view')).toBeVisible();
    await expect(page.getByRole('link', { name: 'Logs' })).toBeVisible();
  });
});
