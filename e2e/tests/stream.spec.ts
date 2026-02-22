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
  verifyCanvasRendering,
} from './test-helpers';

test.describe('Stream View - Dynamic Pipeline', () => {
  let collector: ConsoleErrorCollector;
  // Track the active session ID so afterEach can clean it up via the API
  // even if a test fails mid-way through (e.g. before the UI destroy step).
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

    // Extract the session ID from the page so afterEach can clean it up.
    const sessionIdText = await page.getByText(/Session ID:/).textContent();
    sessionId = sessionIdText?.replace(/Session ID:\s*/, '').trim() ?? null;

    // Assert console errors *before* disconnect/destroy.  Tearing down a MoQ
    // session emits benign WebTransportError noise, so we stop the collector
    // here to avoid false positives.
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

    // Session creation triggers an auto-connect attempt via the UI.  In many
    // environments (especially headless Chromium with self-signed certs) this
    // first attempt fails silently.  We wait for the auto-connect to settle —
    // either the "Relay: connected" label appears, or the "Connect & Stream"
    // button reappears (meaning auto-connect failed and the UI reset).
    const connected = page.getByText('Relay: connected');
    const disconnected = page.getByText('Disconnected');
    const connectButton = page.getByRole('button', {
      name: /Connect & Stream/i,
    });

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

      // Give the subscribe path time to receive, decode, and start playing audio.
      await page.waitForTimeout(2_000);
      // Verify the AudioContext tracker (installed in beforeEach) recorded at
      // least one running context with advancing currentTime — this proves the
      // subscribe side is actually decoding and playing audio frames.
      const audioState = await verifyAudioContextActive(page);
      expect(
        audioState.running,
        'Expected at least one running AudioContext for audio playback'
      ).toBeGreaterThan(0);
      expect(audioState.maxCurrentTime, 'AudioContext should have advanced').toBeGreaterThan(0);

      // Assert console errors before teardown (same pattern as the session
      // lifecycle test — see collector.stop() comment above).
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

  // Safety-net cleanup: if a test fails after creating a session but before
  // destroying it via the UI, delete it through the API so subsequent tests
  // start with a clean slate.
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

test.describe('Stream View - Video MoQ Color Bars Pipeline', () => {
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

  test('creates video session, connects via MoQ, verifies canvas rendering', async ({
    page,
    baseURL,
  }) => {
    test.setTimeout(90_000);

    // Check MoQ gateway availability; skip if not configured.
    const configResponse = await page.request.get(`${baseURL}/api/v1/config`);
    if (configResponse.ok()) {
      const config = (await configResponse.json()) as {
        moq_gateway_url?: string | null;
      };
      if (!config.moq_gateway_url) {
        test.skip(true, 'MoQ gateway not configured on this server');
      }
    }

    // Select the video colorbars MoQ template.
    const templateCard = page.getByText('Video Color Bars (MoQ Stream)', {
      exact: true,
    });
    await expect(templateCard).toBeVisible({ timeout: 10_000 });
    await templateCard.click();

    // Create session.
    const createButton = page.getByRole('button', { name: /Create Session/i });
    await expect(createButton).toBeEnabled({ timeout: 5_000 });
    await createButton.click();

    const activeBadge = page.getByText('Session Active');
    await expect(activeBadge).toBeVisible({ timeout: 15_000 });

    // Extract session ID for cleanup.
    const sessionIdText = await page.getByText(/Session ID:/).textContent();
    sessionId = sessionIdText?.replace(/Session ID:\s*/, '').trim() ?? null;

    // Wait for MoQ connection (auto-connect or manual).
    const connected = page.getByText('Relay: connected');
    const disconnected = page.getByText('Disconnected');
    const connectButton = page.getByRole('button', {
      name: /Connect & Stream/i,
    });

    await expect(connected.or(connectButton)).toBeVisible({ timeout: 20_000 });

    const isConnected = await connected.isVisible();
    if (!isConnected) {
      await expect(connectButton).toBeEnabled({ timeout: 5_000 });
      await connectButton.click();
      await expect(connected.or(disconnected)).toBeVisible({ timeout: 20_000 });
    }

    const finalConnected = await connected.isVisible();
    if (finalConnected) {
      // Wait for the watch path to go live.
      await expect(page.getByText(/Watch: live/)).toBeVisible({
        timeout: 15_000,
      });

      // Give the video decoder time to render a few frames onto the canvas.
      await page.waitForTimeout(3_000);

      // Verify canvas is rendering non-black pixels (SMPTE color bars).
      const canvasState = await verifyCanvasRendering(page);
      expect(canvasState.found, 'Canvas element not found on page').toBe(true);
      expect(canvasState.width, 'Canvas has no width').toBeGreaterThan(0);
      expect(canvasState.height, 'Canvas has no height').toBeGreaterThan(0);
      expect(
        canvasState.hasNonBlackPixels,
        'Canvas should have rendered non-black pixels from color bars'
      ).toBe(true);

      // Assert console errors before teardown.
      const unexpected = collector.getUnexpected(MOQ_BENIGN_PATTERNS);
      expect(unexpected, `Unexpected console errors: ${unexpected.join('; ')}`).toHaveLength(0);
      collector.stop();

      // Disconnect.
      const disconnectButton = page.getByRole('button', { name: /^Disconnect$/i }).first();
      await expect(disconnectButton).toBeVisible();
      await disconnectButton.click();

      await expect(disconnected).toBeVisible({ timeout: 10_000 });
    } else {
      test.skip(true, 'MoQ WebTransport connection could not be established in this environment');
    }

    // Destroy session via UI.
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
        // Best-effort cleanup; ignore errors.
      }
      sessionId = null;
    }
  });
});
