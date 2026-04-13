<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# Running E2E Tests

End-to-end tests live in `e2e/` and use Playwright (Chromium, headless).

## Prerequisites

Before running E2E tests for the first time (or on a fresh checkout), install
dependencies and Playwright browsers:

```bash
just install-e2e          # installs e2e/ Bun dependencies
just install-playwright   # installs headless Chromium
```

See `e2e/README.md` for full details on prerequisites and options.

## Running Tests

1. **Build the UI** and **start the server** in one terminal:

   ```bash
   just build-ui && SK_SERVER__MOQ_GATEWAY_URL=http://127.0.0.1:4545/moq SK_SERVER__ADDRESS=127.0.0.1:4545 just skit
   ```

2. **Run the tests** in a second terminal:

   ```bash
   just e2e-external http://localhost:4545
   ```

## Headless-Browser Pitfalls

- Playwright runs headless Chromium with a default 1280x720 viewport.
  Elements rendered below the fold are **not visible** to
  `IntersectionObserver`. If a test relies on an element being observed
  (e.g. the `<canvas>` used by the MoQ video renderer), scroll it into
  view first:

  ```ts
  const canvas = page.locator('canvas');
  await canvas.scrollIntoViewIfNeeded();
  ```

- The `@moq/watch` `Video.Renderer` enables the `Video.Decoder` (and
  therefore the `video/data` MoQ subscription) **only** when the canvas is
  intersecting. Forgetting to scroll will result in a permanently black
  canvas.
