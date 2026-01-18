// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { test, expect } from '@playwright/test';

import { ensureLoggedIn } from './auth-helpers';

test.describe('About modal', () => {
  test('shows version and build hash when clicking the logo', async ({ page }) => {
    await page.goto('/design');
    await ensureLoggedIn(page);

    const healthResponse = await page.request.get('/healthz');
    expect(healthResponse.ok()).toBeTruthy();
    const health = (await healthResponse.json()) as {
      version?: string;
      build_hash?: string;
      buildHash?: string;
    };

    await page.getByRole('button', { name: 'About StreamKit' }).click();

    const dialog = page.getByRole('dialog', { name: 'About StreamKit' });
    await expect(dialog).toBeVisible();

    const versionValue = health.version ?? 'unknown';
    const buildHashValue = health.build_hash ?? health.buildHash ?? 'unknown';

    await expect(dialog.getByLabel('Version')).toHaveValue(versionValue);
    await expect(dialog.getByLabel('Build hash')).toHaveValue(buildHashValue);

    await dialog.getByRole('button', { name: 'Close' }).click();
    await expect(dialog).toBeHidden();
  });
});
