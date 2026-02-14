// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { test, expect, request } from '@playwright/test';

import { ensureLoggedIn, getAuthHeaders } from './auth-helpers';
import {
  type ConsoleErrorCollector,
  MOQ_BENIGN_PATTERNS,
  createConsoleErrorCollector,
  installAudioContextTracker,
  verifyAudioContextActive,
} from './test-helpers';

test.describe('Stream View - Dynamic Pipeline', () => {
  let collector: ConsoleErrorCollector;
  let sessionId: string | null = null;

  test.beforeEach(async ({ page }) => {
    collector = createConsoleErrorCollector(page);
    await installAudioContextTracker(page);
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

    const templateCard = page.getByText('MoQ Peer Transcoder (Gateway)', {
      exact: true,
    });
    await expect(templateCard).toBeVisible({ timeout: 10_000 });
    await templateCard.click();

    const createButton = page.getByRole('button', { name: /Create Session/i });
    await expect(createButton).toBeEnabled({ timeout: 5_000 });
    await createButton.click();

    const activeBadge = page.getByText('Session Active');
    await expect(activeBadge).toBeVisible({ timeout: 15_000 });

    await expect(page.getByText(/Session ID:/)).toBeVisible();

    const sessionIdText = await page.getByText(/Session ID:/).textContent();
    sessionId = sessionIdText?.replace(/Session ID:\s*/, '').trim() ?? null;

    const unexpected = collector.getUnexpected(MOQ_BENIGN_PATTERNS);
    expect(unexpected, `Unexpected console errors: ${unexpected.join('; ')}`).toHaveLength(0);
    collector.stop();

    const destroyButton = page.getByRole('button', {
      name: /Destroy Session/i,
    });
    await expect(destroyButton).toBeVisible();
    await destroyButton.click();

    const confirmModal = page.getByTestId('confirm-modal');
    await expect(confirmModal).toBeVisible();
    await confirmModal.getByRole('button', { name: /Destroy Session/i }).click();

    await expect(createButton).toBeVisible({ timeout: 15_000 });
    sessionId = null;
  });

  test('connects via MoQ, verifies connection status, then disconnects', async ({
    page,
    baseURL,
  }) => {
    test.setTimeout(60_000);

    const configResponse = await page.request.get(`${baseURL}/api/v1/config`);
    if (configResponse.ok()) {
      const config = (await configResponse.json()) as {
        moq_gateway_url?: string | null;
      };
      if (!config.moq_gateway_url) {
        test.skip(true, 'MoQ gateway not configured on this server');
      }
    }

    const templateCard = page.getByText('MoQ Peer Transcoder (Gateway)', {
      exact: true,
    });
    await expect(templateCard).toBeVisible({ timeout: 10_000 });
    await templateCard.click();

    const createButton = page.getByRole('button', { name: /Create Session/i });
    await expect(createButton).toBeEnabled({ timeout: 5_000 });
    await createButton.click();

    const activeBadge = page.getByText('Session Active');
    await expect(activeBadge).toBeVisible({ timeout: 15_000 });

    const sessionIdText = await page.getByText(/Session ID:/).textContent();
    sessionId = sessionIdText?.replace(/Session ID:\s*/, '').trim() ?? null;

    // Session creation triggers an auto-connect attempt.
    // Wait for it to resolve: either connected or back to disconnected.
    const connected = page.getByText('Relay: connected');
    const disconnected = page.getByText('Disconnected');
    const connectButton = page.getByRole('button', {
      name: /Connect & Stream/i,
    });

    // Wait for the auto-connect to settle.
    await expect(connected.or(connectButton)).toBeVisible({ timeout: 20_000 });

    const isConnected = await connected.isVisible();
    if (!isConnected) {
      // Auto-connect failed (e.g. WebTransport cert issue). Try manual connect.
      await expect(connectButton).toBeEnabled({ timeout: 5_000 });
      await connectButton.click();

      // Wait for either successful connection or failure.
      await expect(connected.or(disconnected)).toBeVisible({ timeout: 20_000 });
    }

    const finalConnected = await connected.isVisible();
    if (finalConnected) {
      await expect(page.getByText(/Watch: live/)).toBeVisible({
        timeout: 15_000,
      });

      await page.waitForTimeout(2_000);
      const audioState = await verifyAudioContextActive(page);
      expect(
        audioState.running,
        'Expected at least one running AudioContext for audio playback'
      ).toBeGreaterThan(0);
      expect(audioState.maxCurrentTime, 'AudioContext should have advanced').toBeGreaterThan(0);

      const unexpected = collector.getUnexpected(MOQ_BENIGN_PATTERNS);
      expect(unexpected, `Unexpected console errors: ${unexpected.join('; ')}`).toHaveLength(0);
      collector.stop();

      const disconnectButton = page.getByRole('button', { name: /^Disconnect$/i }).first();
      await expect(disconnectButton).toBeVisible();
      await disconnectButton.click();

      await expect(disconnected).toBeVisible({ timeout: 10_000 });
    } else {
      test.skip(true, 'MoQ WebTransport connection could not be established in this environment');
    }

    const destroyButton = page.getByRole('button', {
      name: /Destroy Session/i,
    });
    await expect(destroyButton).toBeVisible();
    await destroyButton.click();

    const confirmModal = page.getByTestId('confirm-modal');
    await expect(confirmModal).toBeVisible();
    await confirmModal.getByRole('button', { name: /Destroy Session/i }).click();

    await expect(page.getByRole('button', { name: /Create Session/i })).toBeVisible({
      timeout: 15_000,
    });

    sessionId = null;
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
