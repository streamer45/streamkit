// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { expect, type Page } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';

function readAdminTokenFromStateDir(stateDir: string): string | null {
  const tokenPath = path.join(stateDir, 'admin.token');
  try {
    const token = fs.readFileSync(tokenPath, 'utf8').trim();
    return token || null;
  } catch {
    return null;
  }
}

function discoverAdminTokenFromDisk(): string | null {
  const stateDirCandidates: string[] = [];

  if (process.env.E2E_AUTH_STATE_DIR) {
    stateDirCandidates.push(process.env.E2E_AUTH_STATE_DIR);
  }

  if (process.env.SK_AUTH__STATE_DIR) {
    stateDirCandidates.push(process.env.SK_AUTH__STATE_DIR);
  }

  // Default state dir is ".streamkit/auth" relative to repo root.
  // From e2e/tests/*, repo root is two directories up.
  const __dirname = path.dirname(fileURLToPath(import.meta.url));
  const repoRoot = path.resolve(__dirname, '..', '..');
  stateDirCandidates.push(path.join(repoRoot, '.streamkit', 'auth'));

  for (const stateDir of stateDirCandidates) {
    const token = readAdminTokenFromStateDir(stateDir);
    if (token) return token;
  }

  return null;
}

export const adminToken =
  process.env.E2E_ADMIN_TOKEN?.trim() || discoverAdminTokenFromDisk() || null;

export function getAuthHeaders(): Record<string, string> {
  if (!adminToken) return {};
  return { Authorization: `Bearer ${adminToken}` };
}

/**
 * Logs in via the UI if the login view is currently shown.
 *
 * When auth is disabled, clicks "Continue without auth".
 *
 * Fails with a clear message when auth is enabled but no admin token is available.
 */
export async function ensureLoggedIn(page: Page): Promise<void> {
  const loginView = page.getByTestId('login-view');
  const designView = page.getByTestId('design-view');
  const appViews = [
    designView,
    page.getByTestId('monitor-view'),
    page.getByTestId('convert-view'),
    page.getByTestId('stream-view'),
    page.getByTestId('tokens-view'),
  ];

  // Wait for the app to settle on either:
  // - the login screen, or
  // - any primary app view.
  //
  // When auth is enabled, the app may briefly show a loading spinner before redirecting to /login.
  await Promise.race([
    loginView.waitFor({ state: 'visible', timeout: 30000 }),
    ...appViews.map((view) => view.waitFor({ state: 'visible', timeout: 30000 })),
  ]).catch(() => {
    throw new Error('Timed out waiting for StreamKit UI to show a view (login or app)');
  });

  // If we're already on an app view (design/monitor/convert/stream/tokens), we're done.
  if (!(await loginView.isVisible().catch(() => false))) {
    return;
  }

  const meResponse = await page.request.get('/api/v1/auth/me');
  const meBody = (await meResponse.json()) as { auth_enabled?: boolean; authenticated?: boolean };

  if (meBody.auth_enabled === false) {
    const continueWithoutAuth = page.getByTestId('login-continue-without-auth');
    await expect(continueWithoutAuth).toBeEnabled({ timeout: 20000 });
    await continueWithoutAuth.click();
    await expect(designView).toBeVisible({ timeout: 20000 });
    return;
  }

  if (!adminToken) {
    throw new Error(
      'Login required but admin token is not available. ' +
        'Set E2E_ADMIN_TOKEN or point E2E_AUTH_STATE_DIR/SK_AUTH__STATE_DIR to a directory containing admin.token.'
    );
  }

  // If already authenticated, the app will redirect away from /login automatically.
  if (meBody.authenticated === true) {
    await expect(designView).toBeVisible({ timeout: 20000 });
    return;
  }

  await page.getByTestId('login-token-input').fill(adminToken);
  await page.getByTestId('login-submit').click();

  await expect(designView).toBeVisible({ timeout: 30000 });
}
