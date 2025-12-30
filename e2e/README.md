<!-- SPDX-FileCopyrightText: © 2025 StreamKit Contributors -->
<!-- SPDX-License-Identifier: MPL-2.0 -->

# StreamKit E2E Tests

End-to-end tests for StreamKit using Playwright.

## Prerequisites

- Bun 1.3.5+
- Rust 1.92.0+ (for building skit)
- Built UI (`cd ui && bun install && bun run build` or `just build-ui`)
- Built skit binary (`cargo build -p streamkit-server --bin skit`)

## Quick Start

```bash
# Install dependencies and Playwright browsers
just install-e2e
just install-playwright

# Run tests (automatically starts server)
just e2e

# Or run directly from e2e directory
cd e2e
bun install
bunx playwright install chromium
bun run test
```

## Running Against External Server

If you already have a StreamKit server running:

```bash
E2E_BASE_URL=http://localhost:4545 bun run test:only

# Or via justfile
just e2e-external http://localhost:4545
```

## Running Against Vite Dev Server

To test against the Vite development server (useful for debugging UI changes):

```bash
# Terminal 1: Start skit backend
cargo run -p streamkit-server --bin skit -- serve

# Terminal 2: Start Vite dev server
cd ui && bun run dev

# Terminal 3: Run E2E tests against Vite
just e2e-external http://localhost:3045
```

The Vite dev server proxies `/api/*` and `/healthz` requests to the skit backend
(default `127.0.0.1:4545`). This is primarily for Playwright’s direct API calls when
`E2E_BASE_URL` points at the Vite server; the UI itself still talks directly to the backend
in development (via `import.meta.env.VITE_API_BASE`).

Both servers must be running for tests to pass.

## Test Structure

- `tests/design.spec.ts` - Design view tests (canvas, samples, YAML editor)
- `tests/monitor.spec.ts` - Monitor view tests (session lifecycle)

## Server Harness

When `E2E_BASE_URL` is not set, the test harness (`src/harness/run.ts`):

1. Finds a free port
2. Starts `target/debug/skit serve` with `SK_SERVER__ADDRESS=127.0.0.1:<port>`
3. Polls `/healthz` until server is ready (30s timeout)
4. Runs all Playwright tests
5. Stops the server

Environment variables set by harness:

- `SK_SERVER__ADDRESS` - Bind address
- `SK_LOG__FILE_ENABLE=false` - Disable file logging
- `RUST_LOG=warn` - Reduce log noise

## Scripts

| Script                | Description                                  |
| --------------------- | -------------------------------------------- |
| `bun run test`        | Run tests with auto server management        |
| `bun run test:only`   | Run tests directly (requires `E2E_BASE_URL`) |
| `bun run test:headed` | Run tests with visible browser               |
| `bun run test:ui`     | Run tests with Playwright UI                 |
| `bun run report`      | Show HTML test report                        |

## Debugging

```bash
# Run with debug mode (shows server output)
DEBUG=1 bun run test

# Run single test file
bun run test -- tests/design.spec.ts

# Run with trace viewer on failure
bun run test -- --trace on

# Run specific test by name
bun run test -- -g "loads with all main panes"
```

## CI

Tests run automatically in CI via `.github/workflows/e2e.yml`.
On failure, `playwright-report/` and `test-results/` are uploaded as artifacts.

## Adding New Tests

1. Create a new spec file in `tests/` directory
2. Use `data-testid` attributes for stable element selection
3. Prefer role/name selectors for accessible elements
4. Avoid arbitrary waits; use Playwright's built-in assertions

Example:

```typescript
import { test, expect } from '@playwright/test';

test('my new test', async ({ page }) => {
  await page.goto('/my-route');
  await expect(page.getByTestId('my-element')).toBeVisible();
});
```
