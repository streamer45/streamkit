// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import * as fs from 'fs';
import * as path from 'path';

import { test, expect, request, type Page } from '@playwright/test';

import { ensureLoggedIn, getAuthHeaders } from './auth-helpers';
import { type ConsoleErrorCollector, createConsoleErrorCollector } from './test-helpers';

/**
 * E2E tests for the declarative overlay controls feature.
 *
 * Drives the `overlay-controls.yaml` fixture, which exercises all five control
 * types (toggle, text, number, button, select) using only core nodes
 * (`video::colorbars` → `core::sink`) — no plugins or MoQ gateway needed. The
 * fixture is loaded directly into the Stream view's YAML editor rather than
 * picked from the sample list: it's a test fixture, not a shipped sample.
 */
const OVERLAY_CONTROLS_YAML = fs.readFileSync(
  path.resolve(import.meta.dirname, '../fixtures/overlay-controls.yaml'),
  'utf-8'
);

/**
 * A minimal MoQ pipeline with distinctive broadcast names, used to prove that a
 * direct YAML edit populates the broadcast fields (issue #550) before a second
 * edit clears them.
 */
const MOQ_BROADCAST_YAML = [
  'name: MoQ Derive Probe',
  'mode: dynamic',
  'client:',
  '  gateway_path: /moq/derive-probe',
  '  publish:',
  '    broadcast: derive-in',
  '    tracks:',
  '      - kind: audio',
  '        source: microphone',
  '  watch:',
  '    broadcast: derive-out',
  '    audio: true',
  'nodes:',
  '  colorbars:',
  '    kind: video::colorbars',
  '  sink:',
  '    kind: core::sink',
  '    needs: colorbars',
  '',
].join('\n');

/**
 * Replace the Stream view's pipeline YAML editor contents. The editor is
 * CodeMirror (contenteditable), so we select-all and `insertText` the fixture
 * as a single input event — avoiding per-keystroke autocomplete/auto-indent.
 */
async function setEditorYaml(page: Page, yaml: string): Promise<void> {
  const editor = page.locator('.cm-content');
  await expect(editor).toBeVisible({ timeout: 15_000 });
  await editor.click();
  await page.keyboard.press('ControlOrMeta+A');
  await page.keyboard.press('Delete');
  await page.keyboard.insertText(yaml);
}

/**
 * Load a non-MoQ pipeline into the editor. Editing the YAML directly re-derives
 * MoQ settings (issue #550); the fixture has no MoQ transport, so the broadcast
 * names carried over from the auto-selected first sample must clear. We wait for
 * that before returning so the post-create auto-connect doesn't target a stale
 * broadcast.
 */
async function loadPipelineYaml(page: Page, yaml: string): Promise<void> {
  await setEditorYaml(page, yaml);

  await expect(page.locator('#input-broadcast')).toHaveValue('');
  await expect(page.locator('#output-broadcast')).toHaveValue('');
}

test.describe('Stream View - Overlay Controls', () => {
  let collector: ConsoleErrorCollector;
  let sessionId: string | null = null;

  // Captured WebSocket messages sent from the client.
  let wsSentMessages: unknown[] = [];

  test.beforeEach(async ({ page }) => {
    collector = createConsoleErrorCollector(page);
    wsSentMessages = [];

    // Intercept outgoing WebSocket messages to verify control payloads.
    page.on('websocket', (ws) => {
      ws.on('framesent', (frame) => {
        try {
          const data = JSON.parse(frame.payload as string);
          wsSentMessages.push(data);
        } catch {
          // Ignore non-JSON frames.
        }
      });
    });

    await page.goto('/stream');
    await ensureLoggedIn(page);
    if (!page.url().includes('/stream')) {
      await page.goto('/stream');
    }
    await expect(page.getByTestId('stream-view')).toBeVisible();
  });

  // Regression for #550: a direct YAML edit must re-derive MoQ broadcast
  // settings. Self-contained — drives the populate and clear transitions
  // explicitly rather than relying on whichever sample auto-selects first.
  test('re-derives MoQ broadcasts on direct YAML edits (issue #550)', async ({ page }) => {
    await setEditorYaml(page, MOQ_BROADCAST_YAML);
    await expect(page.locator('#input-broadcast')).toHaveValue('derive-in');
    await expect(page.locator('#output-broadcast')).toHaveValue('derive-out');

    await setEditorYaml(page, OVERLAY_CONTROLS_YAML);
    await expect(page.locator('#input-broadcast')).toHaveValue('');
    await expect(page.locator('#output-broadcast')).toHaveValue('');
  });

  test('renders all control types and sends correct UpdateParams on interaction', async ({
    page,
  }) => {
    test.setTimeout(60_000);
    await loadPipelineYaml(page, OVERLAY_CONTROLS_YAML);
    const createButton = page.getByRole('button', { name: /Create Session/i });
    await expect(createButton).toBeEnabled({ timeout: 5_000 });
    await createButton.click();

    const activeBadge = page.getByText('Session Active');
    await expect(activeBadge).toBeVisible({ timeout: 15_000 });

    const sessionIdText = await page.getByText(/Session ID:/).textContent();
    sessionId = sessionIdText?.replace(/Session ID:\s*/, '').trim() ?? null;
    const controls = page.getByTestId('overlay-controls');
    await expect(controls).toBeVisible({ timeout: 5_000 });
    await expect(controls.getByText('Pipeline Controls', { exact: true })).toBeVisible();
    // Scope all locators to the controls section to avoid collisions
    // with the YAML editor that also displays control label strings.
    // Use label locators to avoid collisions with button text.
    const labels = controls.locator('label');
    await expect(labels.filter({ hasText: 'Draw Time' })).toBeVisible();
    await expect(labels.filter({ hasText: 'Label' })).toBeVisible();
    await expect(labels.filter({ hasText: 'Width' })).toBeVisible();
    await expect(labels.filter({ hasText: 'Height' })).toBeVisible();
    await expect(labels.filter({ hasText: 'Format' })).toBeVisible();
    await expect(labels.filter({ hasText: 'Reset' })).toBeVisible();

    // Verify group heading.
    await expect(controls.getByText('Dimensions', { exact: true })).toBeVisible();
    // The toggle defaults to true (checked).  Click it to toggle off.
    const toggleButton = controls.locator('button[aria-label="Draw Time"]');
    await expect(toggleButton).toBeVisible();
    await toggleButton.click();

    // Wait for the WS message to be sent.
    await page.waitForTimeout(200);

    // Find the TuneNodeAsync message for the toggle.
    const toggleMsg = wsSentMessages.find(
      (m: unknown) =>
        typeof m === 'object' &&
        m !== null &&
        (m as Record<string, unknown>).type === 'request' &&
        ((m as Record<string, Record<string, unknown>>).payload?.action === 'tunenodeasync' ||
          (m as Record<string, Record<string, unknown>>).payload?.action === 'TuneNodeAsync') &&
        (m as Record<string, Record<string, unknown>>).payload?.node_id === 'colorbars'
    );
    expect(toggleMsg, 'Expected a TuneNodeAsync message for the toggle control').toBeTruthy();

    const togglePayload = (toggleMsg as Record<string, Record<string, Record<string, unknown>>>)
      .payload?.message?.UpdateParams;
    expect(togglePayload, 'Toggle should send { draw_time: false }').toEqual({
      draw_time: false,
    });
    const textInput = controls.locator('input[placeholder="Label"]');
    await expect(textInput).toBeVisible();
    // Clear the default value and type a new one.
    await textInput.fill('World');

    // Text is debounced at 300ms — wait for it to fire.
    await page.waitForTimeout(500);

    const textMsg = wsSentMessages.find(
      (m: unknown) =>
        typeof m === 'object' &&
        m !== null &&
        (m as Record<string, unknown>).type === 'request' &&
        ((m as Record<string, Record<string, unknown>>).payload?.action === 'tunenodeasync' ||
          (m as Record<string, Record<string, unknown>>).payload?.action === 'TuneNodeAsync') &&
        (m as Record<string, Record<string, unknown>>).payload?.node_id === 'colorbars' &&
        typeof (
          (m as Record<string, Record<string, Record<string, unknown>>>).payload?.message
            ?.UpdateParams as Record<string, unknown>
        )?.label === 'string'
    );
    expect(textMsg, 'Expected a TuneNodeAsync message for the text control').toBeTruthy();

    const textPayload = (textMsg as Record<string, Record<string, Record<string, unknown>>>).payload
      ?.message?.UpdateParams;
    expect(textPayload, 'Text should send { label: "World" }').toEqual({
      label: 'World',
    });
    // Clear previous messages to isolate slider messages.
    wsSentMessages.length = 0;

    const slider = controls.locator('input[type="range"]').first();
    await expect(slider).toBeVisible();

    // Set the slider to a specific value via fill (simulates user input).
    await slider.fill('800');

    // Throttled — wait for trailing edge.
    await page.waitForTimeout(300);

    const sliderMsg = wsSentMessages.find(
      (m: unknown) =>
        typeof m === 'object' &&
        m !== null &&
        (m as Record<string, unknown>).type === 'request' &&
        ((m as Record<string, Record<string, unknown>>).payload?.action === 'tunenodeasync' ||
          (m as Record<string, Record<string, unknown>>).payload?.action === 'TuneNodeAsync') &&
        (m as Record<string, Record<string, unknown>>).payload?.node_id === 'colorbars' &&
        (
          (m as Record<string, Record<string, Record<string, unknown>>>).payload?.message
            ?.UpdateParams as Record<string, unknown>
        )?.properties !== undefined
    );
    expect(sliderMsg, 'Expected a TuneNodeAsync message for the slider control').toBeTruthy();

    const sliderPayload = (sliderMsg as Record<string, Record<string, Record<string, unknown>>>)
      .payload?.message?.UpdateParams;
    // Width slider: dot-notation "properties.width" → nested { properties: { width: 800 } }
    expect(sliderPayload).toHaveProperty('properties');
    expect((sliderPayload as Record<string, Record<string, unknown>>).properties).toHaveProperty(
      'width'
    );
    wsSentMessages.length = 0;

    const selectDropdown = controls.locator('select[aria-label="Format"]');
    await expect(selectDropdown).toBeVisible();
    // Select the second option (NV12).
    await selectDropdown.selectOption({ index: 1 });

    await page.waitForTimeout(200);

    const selectMsg = wsSentMessages.find(
      (m: unknown) =>
        typeof m === 'object' &&
        m !== null &&
        (m as Record<string, unknown>).type === 'request' &&
        ((m as Record<string, Record<string, unknown>>).payload?.action === 'tunenodeasync' ||
          (m as Record<string, Record<string, unknown>>).payload?.action === 'TuneNodeAsync') &&
        (m as Record<string, Record<string, unknown>>).payload?.node_id === 'colorbars' &&
        (
          (m as Record<string, Record<string, Record<string, unknown>>>).payload?.message
            ?.UpdateParams as Record<string, unknown>
        )?.pixel_format === 'nv12'
    );
    expect(selectMsg, 'Expected a TuneNodeAsync message for the select control').toBeTruthy();
    wsSentMessages.length = 0;

    const resetButton = controls.getByRole('button', { name: 'Reset' });
    await expect(resetButton).toBeVisible();
    await resetButton.click();

    await page.waitForTimeout(200);

    const buttonMsg = wsSentMessages.find(
      (m: unknown) =>
        typeof m === 'object' &&
        m !== null &&
        (m as Record<string, unknown>).type === 'request' &&
        ((m as Record<string, Record<string, unknown>>).payload?.action === 'tunenodeasync' ||
          (m as Record<string, Record<string, unknown>>).payload?.action === 'TuneNodeAsync') &&
        (m as Record<string, Record<string, unknown>>).payload?.node_id === 'colorbars' &&
        (
          (m as Record<string, Record<string, Record<string, unknown>>>).payload?.message
            ?.UpdateParams as Record<string, unknown>
        )?.reset === true
    );
    expect(buttonMsg, 'Expected a TuneNodeAsync message for the button control').toBeTruthy();
    const unexpected = collector.getUnexpected();
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

  // Safety-net cleanup.
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
