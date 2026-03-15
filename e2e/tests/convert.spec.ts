// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import * as fs from 'fs';
import * as path from 'path';

import { test, expect } from '@playwright/test';

import { ensureLoggedIn, getAuthHeaders } from './auth-helpers';
import {
  type ConsoleErrorCollector,
  createConsoleErrorCollector,
  verifyAudioPlayback,
  verifyVideoPlayback,
} from './test-helpers';

const repoRoot = path.resolve(import.meta.dirname, '..', '..');
const sampleOggPath = path.join(repoRoot, 'samples', 'audio', 'system', 'sample.ogg');
const mixingYaml = fs.readFileSync(
  path.join(repoRoot, 'samples', 'pipelines', 'oneshot', 'mixing.yml'),
  'utf8'
);

test.describe('Convert View - Audio Mixing Pipeline', () => {
  let collector: ConsoleErrorCollector;

  test.beforeEach(async ({ page }) => {
    collector = createConsoleErrorCollector(page);
    await page.goto('/convert');
    await ensureLoggedIn(page);
    if (!page.url().includes('/convert')) {
      await page.goto('/convert');
    }
    await expect(page.getByTestId('convert-view')).toBeVisible();
  });

  // This test runs the mixing pipeline at the API level to verify the server
  // can process audio.  We use page.evaluate() to issue the request from the
  // browser context so it shares the same origin/cookies, and we read only the
  // *first chunk* of the streamed response before cancelling.  This avoids a
  // timeout: the mixing pipeline streams output in real-time, so waiting for
  // the full body would take as long as the audio duration.
  test('API: POST /api/v1/process with mixing pipeline returns audio', async ({
    page,
    baseURL,
  }) => {
    const audioBase64 = fs.readFileSync(sampleOggPath).toString('base64');
    const authHeaders = getAuthHeaders();

    const result = await page.evaluate(
      async ({ url, yaml, audio, headers }) => {
        const formData = new FormData();
        formData.append('config', yaml);
        const bytes = Uint8Array.from(atob(audio), (c) => c.charCodeAt(0));
        formData.append('media', new Blob([bytes], { type: 'audio/ogg' }), 'sample.ogg');

        const controller = new AbortController();
        const timeoutId = setTimeout(() => controller.abort(), 30_000);

        try {
          const response = await fetch(`${url}/api/v1/process`, {
            method: 'POST',
            body: formData,
            headers,
            signal: controller.signal,
          });

          const contentType = response.headers.get('content-type') ?? '';
          // Read only the first chunk to confirm audio is being produced,
          // then cancel the stream to avoid waiting for real-time playback.
          const reader = response.body!.getReader();
          const { value } = await reader.read();
          reader.cancel();

          return {
            status: response.status,
            contentType,
            firstChunkSize: value?.length ?? 0,
          };
        } finally {
          clearTimeout(timeoutId);
        }
      },
      {
        url: baseURL,
        yaml: mixingYaml,
        audio: audioBase64,
        headers: authHeaders,
      }
    );

    expect(result.status, `Process request failed: ${result.status}`).toBe(200);
    expect(
      result.contentType.includes('audio/') ||
        result.contentType.includes('video/webm') ||
        result.contentType.includes('application/octet'),
      `Unexpected Content-Type: ${result.contentType}`
    ).toBeTruthy();
    expect(result.firstChunkSize).toBeGreaterThan(0);
  });

  test('UI: select mixing template, upload file, convert, verify audio player', async ({
    page,
  }) => {
    await expect(page.getByText('1. Select Pipeline Template')).toBeVisible();

    const templateCard = page.getByText('Audio Mixing (Upload + Music Track)', {
      exact: true,
    });
    await expect(templateCard).toBeVisible({ timeout: 10_000 });
    await templateCard.click();

    await expect(page.locator('input[type="file"]').first()).toBeAttached();
    await page.locator('input[type="file"]').first().setInputFiles(sampleOggPath);

    await expect(page.getByText('sample.ogg')).toBeVisible();

    const convertButton = page.getByRole('button', { name: /Convert File/i });
    await expect(convertButton).toBeEnabled();
    await convertButton.click();

    await expect(page.getByText('Converted Audio')).toBeVisible({
      timeout: 60_000,
    });

    const playback = await verifyAudioPlayback(page);
    expect(playback.found, 'Audio element not found on page').toBe(true);
    expect(playback.duration, 'Audio has no duration').toBeGreaterThan(0);

    const unexpected = collector.getUnexpected();
    expect(unexpected, `Unexpected console errors: ${unexpected.join('; ')}`).toHaveLength(0);
  });

  test('UI: select mixing template, use existing asset, convert, verify audio player', async ({
    page,
  }) => {
    await expect(page.getByText('1. Select Pipeline Template')).toBeVisible();

    const templateCard = page.getByText('Audio Mixing (Upload + Music Track)', {
      exact: true,
    });
    await expect(templateCard).toBeVisible({ timeout: 10_000 });
    await templateCard.click();

    const assetModeButton = page.getByRole('button', {
      name: /Select Existing Asset/i,
    });
    await expect(assetModeButton).toBeVisible();
    await assetModeButton.click();

    const assetRadioGroup = page.locator('[aria-label="Audio asset selection"]');
    await expect(assetRadioGroup).toBeVisible({ timeout: 10_000 });

    const firstAsset = assetRadioGroup.locator('label').first();
    await expect(firstAsset).toBeVisible();
    await firstAsset.click();

    const convertButton = page.getByRole('button', { name: /Convert File/i });
    await expect(convertButton).toBeEnabled();
    await convertButton.click();

    await expect(page.getByText('Converted Audio')).toBeVisible({
      timeout: 60_000,
    });

    const playback = await verifyAudioPlayback(page);
    expect(playback.found, 'Audio element not found on page').toBe(true);
    expect(playback.duration, 'Audio has no duration').toBeGreaterThan(0);

    const unexpected = collector.getUnexpected();
    expect(unexpected, `Unexpected console errors: ${unexpected.join('; ')}`).toHaveLength(0);
  });
});

test.describe('Convert View - Video Color Bars Pipeline', () => {
  let collector: ConsoleErrorCollector;

  test.beforeEach(async ({ page }) => {
    collector = createConsoleErrorCollector(page);
    await page.goto('/convert');
    await ensureLoggedIn(page);
    if (!page.url().includes('/convert')) {
      await page.goto('/convert');
    }
    await expect(page.getByTestId('convert-view')).toBeVisible();
  });

  test('UI: select video colorbars template, generate, verify video player', async ({ page }) => {
    // VP9 encoding can be slow; give the full pipeline up to 120s.
    test.setTimeout(120_000);

    await expect(page.getByText('1. Select Pipeline Template')).toBeVisible();

    const templateCard = page.getByText('Video Color Bars (VP9/WebM)', {
      exact: true,
    });
    await expect(templateCard).toBeVisible({ timeout: 10_000 });
    await templateCard.click();

    // This is a no-input (generator) pipeline, so the button says "Generate".
    const generateButton = page.getByRole('button', { name: /Generate/i });
    await expect(generateButton).toBeEnabled();
    await generateButton.click();

    // Wait for the video output to appear.
    await expect(page.getByText('Converted Video')).toBeVisible({
      timeout: 90_000,
    });

    const playback = await verifyVideoPlayback(page);
    expect(playback.found, 'Video element not found on page').toBe(true);
    expect(playback.readyState, 'Video not ready').toBeGreaterThanOrEqual(1);
    expect(playback.videoWidth, 'Video has no width').toBeGreaterThan(0);
    expect(playback.videoHeight, 'Video has no height').toBeGreaterThan(0);

    const unexpected = collector.getUnexpected();
    expect(unexpected, `Unexpected console errors: ${unexpected.join('; ')}`).toHaveLength(0);
  });
});
