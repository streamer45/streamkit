// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { defineConfig, devices } from '@playwright/test';

// E2E_BASE_URL is set by the harness runner (run.ts) or passed externally
const baseURL = process.env.E2E_BASE_URL;

if (!baseURL) {
  throw new Error(
    'E2E_BASE_URL environment variable is required. ' +
      'Run tests via "bun run test" to auto-start the server, ' +
      'or set E2E_BASE_URL manually for an external server.'
  );
}

export default defineConfig({
  testDir: './tests',
  fullyParallel: false, // Run tests serially for shared server
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: 1, // Single worker for shared server state
  reporter: process.env.CI ? [['html'], ['github']] : [['html']],
  timeout: 30000,
  expect: {
    timeout: 10000,
  },

  use: {
    baseURL,
    trace: 'on-first-retry',
    screenshot: 'only-on-failure',
    video: 'retain-on-failure',
  },

  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
});
