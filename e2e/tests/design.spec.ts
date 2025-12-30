// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { test, expect } from '@playwright/test';

test.describe('Design View', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/design');
    // Wait for the design view to load
    await expect(page.getByTestId('design-view')).toBeVisible();
  });

  test('loads with all main panes visible', async ({ page }) => {
    // Left pane (Control Pane / Library)
    await expect(page.getByTestId('control-pane')).toBeVisible();

    // Center pane (Flow Canvas)
    await expect(page.getByTestId('flow-canvas')).toBeVisible();

    // Right pane (YAML Pane)
    await expect(page.getByTestId('yaml-pane')).toBeVisible();
  });

  test('sample pipelines pane shows samples', async ({ page }) => {
    // Click on Samples tab in the control pane
    await page.getByTestId('samples-tab').click();

    // Wait for samples pane to be visible
    await expect(page.getByTestId('samples-pane')).toBeVisible();

    // Verify at least one sample card is visible (system samples should always exist)
    await expect(page.getByTestId('sample-card').first()).toBeVisible({
      timeout: 10000,
    });
  });

  test('loading a oneshot sample populates the canvas and YAML editor', async ({ page }) => {
    // Ensure we're in oneshot mode (default is dynamic)
    await page.getByRole('button', { name: /Oneshot/i }).click();

    // Open samples pane
    await page.getByTestId('samples-tab').click();
    await expect(page.getByTestId('samples-pane')).toBeVisible();

    // Wait for samples list to populate and find "Volume Boost" sample
    const volumeBoostSample = page.getByTestId('sample-card').filter({ hasText: 'Volume Boost' });
    await expect(volumeBoostSample).toBeVisible({ timeout: 10000 });

    // Click the card's load button (avoid clicking the container div)
    await volumeBoostSample.getByRole('button').click();

    // Handle confirmation modal if canvas has content
    const confirmModal = page.getByTestId('confirm-modal');
    if (await confirmModal.isVisible({ timeout: 2000 }).catch(() => false)) {
      // Click the confirm/load button (scope to modal)
      await confirmModal.getByRole('button', { name: /Load Sample|Load|Confirm/i }).click();
    }

    // Verify nodes appear on canvas (React Flow renders nodes with this class)
    await expect(page.locator('.react-flow__node').first()).toBeVisible({
      timeout: 10000,
    });

    // Verify YAML pane shows the loaded pipeline
    const yamlPane = page.getByTestId('yaml-pane');
    await expect(yamlPane).toBeVisible();

    // Check that YAML contains expected mode indicator
    // The CodeMirror editor renders content in .cm-content
    await expect(page.locator('.cm-content')).toContainText('mode: oneshot');
  });
});
