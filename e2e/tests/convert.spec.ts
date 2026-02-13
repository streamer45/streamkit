// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import * as fs from 'fs';
import * as path from 'path';

import { test, expect, request } from '@playwright/test';

import { ensureLoggedIn, getAuthHeaders } from './auth-helpers';

const repoRoot = path.resolve(import.meta.dirname, '..', '..');
const sampleOggPath = path.join(repoRoot, 'samples', 'audio', 'system', 'sample.ogg');
const mixingYaml = fs.readFileSync(
  path.join(repoRoot, 'samples', 'pipelines', 'oneshot', 'mixing.yml'),
  'utf8'
);

test.describe('Convert View - Audio Mixing Pipeline', () => {
  const consoleErrors: string[] = [];

  test.beforeEach(async ({ page }) => {
    consoleErrors.length = 0;
    page.on('console', (msg) => {
      if (msg.type() === 'error') {
        consoleErrors.push(msg.text());
      }
    });
    await page.goto('/convert');
    await ensureLoggedIn(page);
    if (!page.url().includes('/convert')) {
      await page.goto('/convert');
    }
    await expect(page.getByTestId('convert-view')).toBeVisible();
  });

  test('API: POST /api/v1/process with mixing pipeline returns audio', async ({ baseURL }) => {
    const apiContext = await request.newContext({
      baseURL: baseURL!,
      extraHTTPHeaders: getAuthHeaders(),
    });

    try {
      const response = await apiContext.post('/api/v1/process', {
        multipart: {
          config: mixingYaml,
          media: {
            name: 'sample.ogg',
            mimeType: 'audio/ogg',
            buffer: fs.readFileSync(sampleOggPath),
          },
        },
        timeout: 60_000,
      });

      const responseBody = await response.body();
      expect(response.ok(), `Process request failed: ${response.status()}`).toBeTruthy();

      const ct = response.headers()['content-type'] ?? '';
      expect(
        ct.includes('audio/') || ct.includes('video/webm') || ct.includes('application/octet'),
        `Unexpected Content-Type: ${ct}`
      ).toBeTruthy();

      expect(responseBody.length).toBeGreaterThan(1000);
    } finally {
      await apiContext.dispose();
    }
  });

  test('UI: select mixing template, upload file, convert, verify audio player', async ({
    page,
  }) => {
    await expect(page.getByText('1. Select Pipeline Template')).toBeVisible();

    const templateCard = page.getByText('Audio Mixing (Upload + Music Track)');
    await expect(templateCard).toBeVisible({ timeout: 10_000 });
    await templateCard.click();

    await expect(page.locator('input[type="file"]').first()).toBeAttached();
    await page.locator('input[type="file"]').first().setInputFiles(sampleOggPath);

    await expect(page.getByText('sample.ogg')).toBeVisible();

    const convertButton = page.getByRole('button', { name: /Convert File/i });
    await expect(convertButton).toBeEnabled();
    await convertButton.click();

    await expect(page.getByText('Converted Audio')).toBeVisible({ timeout: 60_000 });

    const unexpected = consoleErrors.filter(
      (msg) => !msg.includes('ResizeObserver') && !msg.includes('WebSocket')
    );
    expect(unexpected, `Unexpected console errors: ${unexpected.join('; ')}`).toHaveLength(0);
  });

  test('UI: select mixing template, use existing asset, convert, verify audio player', async ({
    page,
  }) => {
    await expect(page.getByText('1. Select Pipeline Template')).toBeVisible();

    const templateCard = page.getByText('Audio Mixing (Upload + Music Track)');
    await expect(templateCard).toBeVisible({ timeout: 10_000 });
    await templateCard.click();

    const assetModeButton = page.getByRole('button', { name: /Select Existing Asset/i });
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

    await expect(page.getByText('Converted Audio')).toBeVisible({ timeout: 60_000 });

    const unexpected = consoleErrors.filter(
      (msg) => !msg.includes('ResizeObserver') && !msg.includes('WebSocket')
    );
    expect(unexpected, `Unexpected console errors: ${unexpected.join('; ')}`).toHaveLength(0);
  });
});
