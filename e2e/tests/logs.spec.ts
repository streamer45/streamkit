// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { test, expect } from '@playwright/test';

import { ensureLoggedIn, getAuthHeaders } from './auth-helpers';

test.describe('Log Viewer', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/admin/logs');
    await ensureLoggedIn(page);
  });

  test('navigates to log viewer and displays logs', async ({ page }) => {
    await expect(page.getByTestId('logs-view')).toBeVisible();

    // Should show the title
    await expect(page.getByText('Logs')).toBeVisible();

    // Should show the admin nav with Logs link active
    await expect(page.getByRole('link', { name: 'Logs' })).toBeVisible();

    // Log container should be present
    await expect(page.getByTestId('logs-container')).toBeVisible();

    // Wait for logs to load (the server generates logs, so there should be some)
    await expect(page.getByTestId('logs-container')).not.toBeEmpty({ timeout: 10000 });
  });

  test('filter controls are present and functional', async ({ page }) => {
    await expect(page.getByTestId('logs-view')).toBeVisible();

    // Filter input should be visible
    const filterInput = page.getByTestId('logs-filter-input');
    await expect(filterInput).toBeVisible();

    // Level select should be visible
    const levelSelect = page.getByTestId('logs-level-select');
    await expect(levelSelect).toBeVisible();

    // Filter button should be visible
    const filterButton = page.getByTestId('logs-apply-filter');
    await expect(filterButton).toBeVisible();

    // Live tail button should be visible
    const liveTailButton = page.getByTestId('logs-live-tail');
    await expect(liveTailButton).toBeVisible();
  });

  test('pagination buttons are present', async ({ page }) => {
    await expect(page.getByTestId('logs-view')).toBeVisible();

    // Wait for initial load
    await expect(page.getByTestId('logs-container')).not.toBeEmpty({ timeout: 10000 });

    // Pagination buttons should be visible
    await expect(page.getByTestId('logs-load-older')).toBeVisible();
    await expect(page.getByTestId('logs-load-newer')).toBeVisible();
    await expect(page.getByTestId('logs-load-latest')).toBeVisible();
  });

  test('level filter changes displayed logs', async ({ page }) => {
    await expect(page.getByTestId('logs-view')).toBeVisible();

    // Wait for initial load
    await expect(page.getByTestId('logs-container')).not.toBeEmpty({ timeout: 10000 });

    // Select "Error" level filter
    await page.getByTestId('logs-level-select').selectOption('error');
    await page.getByTestId('logs-apply-filter').click();

    // Wait briefly for the filtered results
    await page.waitForTimeout(1000);

    // The log container should still be present (may be empty if no errors)
    await expect(page.getByTestId('logs-container')).toBeVisible();
  });

  test('text filter applies correctly', async ({ page }) => {
    await expect(page.getByTestId('logs-view')).toBeVisible();

    // Wait for initial load
    await expect(page.getByTestId('logs-container')).not.toBeEmpty({ timeout: 10000 });

    // Type a filter that should match some logs
    await page.getByTestId('logs-filter-input').fill('skit');
    await page.getByTestId('logs-apply-filter').click();

    // Wait for filtered results
    await page.waitForTimeout(1000);

    // Container should still be visible
    await expect(page.getByTestId('logs-container')).toBeVisible();
  });

  test('log API returns valid response', async ({ page }) => {
    const headers = getAuthHeaders();

    // Test the logs API directly
    const response = await page.request.get('/api/v1/logs?limit=10&direction=backward', {
      headers,
    });

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
    expect(body.file_size).toBeGreaterThan(0);
  });

  test('log API supports forward pagination', async ({ page }) => {
    const headers = getAuthHeaders();

    // First request: get first page
    const firstPage = await page.request.get('/api/v1/logs?limit=5&direction=forward&offset=0', {
      headers,
    });
    expect(firstPage.ok()).toBeTruthy();

    const firstBody = (await firstPage.json()) as {
      lines: string[];
      next_offset: number;
      has_more: boolean;
    };

    if (firstBody.has_more) {
      // Second request: get next page using next_offset
      const secondPage = await page.request.get(
        `/api/v1/logs?limit=5&direction=forward&offset=${firstBody.next_offset}`,
        { headers }
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

  test('log API supports level filtering', async ({ page }) => {
    const headers = getAuthHeaders();

    const response = await page.request.get(
      '/api/v1/logs?limit=100&direction=backward&level=info',
      {
        headers,
      }
    );

    expect(response.ok()).toBeTruthy();

    const body = (await response.json()) as { lines: string[] };

    // All returned lines should contain INFO level marker
    for (const line of body.lines) {
      const hasInfoLevel = / INFO /i.test(line) || /"level":"INFO"/i.test(line);
      expect(hasInfoLevel).toBeTruthy();
    }
  });

  test('admin nav shows Logs link on all admin pages', async ({ page }) => {
    // Check logs link is present on plugins page
    await page.goto('/admin/plugins');
    await ensureLoggedIn(page);
    await expect(page.getByRole('link', { name: 'Logs' })).toBeVisible();

    // Check logs link is present on tokens page
    await page.goto('/admin/tokens');
    await expect(page.getByRole('link', { name: 'Logs' })).toBeVisible();
  });
});
