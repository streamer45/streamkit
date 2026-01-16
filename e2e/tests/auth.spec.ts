// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { test, expect } from '@playwright/test';

import { adminToken, ensureLoggedIn } from './auth-helpers';

test.describe('Auth Flow', () => {
  test('requires auth for API and redirects UI to login', async ({ page }) => {
    const me = await page.request.get('/api/v1/auth/me');
    const meBody = (await me.json()) as { auth_enabled: boolean };
    test.skip(!meBody.auth_enabled, 'Auth is disabled for this server');
    test.skip(
      !adminToken,
      'Admin token not available (set E2E_ADMIN_TOKEN or generate admin.token)'
    );

    const unauthenticatedList = await page.request.get('/api/v1/sessions');
    expect(unauthenticatedList.status()).toBe(401);

    await page.goto('/design');
    await expect(page.getByTestId('login-view')).toBeVisible();
  });

  test('signs in with token, grants cookie access, and supports logout', async ({ page }) => {
    const me = await page.request.get('/api/v1/auth/me');
    const meBody = (await me.json()) as { auth_enabled: boolean };
    test.skip(!meBody.auth_enabled, 'Auth is disabled for this server');
    test.skip(
      !adminToken,
      'Admin token not available (set E2E_ADMIN_TOKEN or generate admin.token)'
    );

    await page.goto('/design');
    await expect(page.getByTestId('login-view')).toBeVisible();

    await ensureLoggedIn(page);

    await expect(page.getByTestId('design-view')).toBeVisible();
    await expect(page.getByRole('link', { name: 'Admin' })).toBeVisible();

    const authenticatedList = await page.request.get('/api/v1/sessions');
    expect(authenticatedList.ok()).toBeTruthy();

    await page.goto('/admin/tokens');
    await expect(page.getByTestId('tokens-view')).toBeVisible();

    await page.getByTestId('tokens-logout').click();
    await expect(page.getByTestId('login-view')).toBeVisible();

    const loggedOutList = await page.request.get('/api/v1/sessions');
    expect(loggedOutList.status()).toBe(401);
  });
});
