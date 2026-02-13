// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { test, expect, request } from '@playwright/test';

import { ensureLoggedIn, getAuthHeaders } from './auth-helpers';

test.describe('Stream View - Dynamic Pipeline', () => {
  const consoleErrors: string[] = [];
  let sessionId: string | null = null;

  test.beforeEach(async ({ page }) => {
    consoleErrors.length = 0;
    page.on('console', (msg) => {
      if (msg.type() === 'error') {
        consoleErrors.push(msg.text());
      }
    });
    await page.goto('/stream');
    await ensureLoggedIn(page);
    if (!page.url().includes('/stream')) {
      await page.goto('/stream');
    }
    await expect(page.getByTestId('stream-view')).toBeVisible();
  });

  test('creates session from template, verifies active badge, then destroys it', async ({
    page,
  }) => {
    const pipelineHeading = page.getByText('Pipeline Selection');
    await expect(pipelineHeading).toBeVisible({ timeout: 15_000 });

    const templateCard = page.getByText('MoQ Peer Transcoder (Gateway)');
    await expect(templateCard).toBeVisible({ timeout: 10_000 });
    await templateCard.click();

    const createButton = page.getByRole('button', { name: /Create Session/i });
    await expect(createButton).toBeEnabled({ timeout: 5_000 });
    await createButton.click();

    const activeBadge = page.getByText('Session Active');
    await expect(activeBadge).toBeVisible({ timeout: 15_000 });

    await expect(page.getByText(/Session ID:/)).toBeVisible();

    const destroyButton = page.getByRole('button', { name: /Destroy Session/i });
    await expect(destroyButton).toBeVisible();
    await destroyButton.click();

    const confirmModal = page.getByTestId('confirm-modal');
    await expect(confirmModal).toBeVisible();
    await confirmModal.getByRole('button', { name: /Destroy Session/i }).click();

    await expect(createButton).toBeVisible({ timeout: 15_000 });

    const unexpected = consoleErrors.filter(
      (msg) => !msg.includes('ResizeObserver') && !msg.includes('WebSocket')
    );
    expect(unexpected, `Unexpected console errors: ${unexpected.join('; ')}`).toHaveLength(0);
  });

  test('connects via MoQ, verifies connection status, then disconnects', async ({
    page,
    baseURL,
  }) => {
    const configResponse = await page.request.get(`${baseURL}/api/v1/config`);
    if (configResponse.ok()) {
      const config = (await configResponse.json()) as { moq_gateway_url?: string | null };
      if (!config.moq_gateway_url) {
        test.skip(true, 'MoQ gateway not configured on this server');
      }
    }

    const templateCard = page.getByText('MoQ Peer Transcoder (Gateway)');
    await expect(templateCard).toBeVisible({ timeout: 10_000 });
    await templateCard.click();

    const createButton = page.getByRole('button', { name: /Create Session/i });
    await expect(createButton).toBeEnabled({ timeout: 5_000 });
    await createButton.click();

    const activeBadge = page.getByText('Session Active');
    await expect(activeBadge).toBeVisible({ timeout: 15_000 });

    const connectButton = page.getByRole('button', { name: /Connect & Stream/i });
    await expect(connectButton).toBeEnabled({ timeout: 5_000 });
    await connectButton.click();

    await expect(page.getByText('Relay: connected')).toBeVisible({ timeout: 20_000 });

    await expect(page.getByText(/Watch: live/)).toBeVisible({ timeout: 15_000 });

    const disconnectButton = page.getByRole('button', { name: /^Disconnect$/i });
    await expect(disconnectButton).toBeVisible();
    await disconnectButton.click();

    await expect(page.getByText('Disconnected')).toBeVisible({ timeout: 10_000 });

    const destroyButton = page.getByRole('button', { name: /Destroy Session/i });
    await expect(destroyButton).toBeVisible();
    await destroyButton.click();

    const confirmModal = page.getByTestId('confirm-modal');
    await expect(confirmModal).toBeVisible();
    await confirmModal.getByRole('button', { name: /Destroy Session/i }).click();

    await expect(page.getByRole('button', { name: /Create Session/i })).toBeVisible({
      timeout: 15_000,
    });

    const unexpected = consoleErrors.filter(
      (msg) =>
        !msg.includes('ResizeObserver') &&
        !msg.includes('WebSocket') &&
        !msg.includes('WebTransport') &&
        !msg.includes('ERR_QUIC_PROTOCOL_ERROR')
    );
    expect(unexpected, `Unexpected console errors: ${unexpected.join('; ')}`).toHaveLength(0);
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
        // Ignore cleanup errors
      }
      sessionId = null;
    }
  });
});
