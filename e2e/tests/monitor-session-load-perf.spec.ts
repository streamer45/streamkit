// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Layer 2 — Monitor view session-load perf test.
 *
 * Creates a multi-node pipeline session via the API, navigates to the monitor
 * view, selects the session, and measures re-renders that occur during the
 * initial session load (pipeline hydration + WebSocket state events).
 *
 * The test asserts that:
 *   - MonitorView renders stay within a reasonable budget during session load.
 *   - The batching optimisations prevent cascade re-renders from individual
 *     WebSocket events flooding the Zustand store with separate set() calls.
 *
 * NOTE: This test requires the Vite dev server (`just ui`) because the
 * profiler store (`window.__PERF_DATA__`) is only exposed when
 * `import.meta.env.DEV` is true.  Point E2E_BASE_URL at
 * http://localhost:3045 (or wherever the dev server runs).
 */

import { test, expect, request } from "@playwright/test";

import { ensureLoggedIn, getAuthHeaders } from "./auth-helpers";
import {
  type ConsoleErrorCollector,
  MOQ_BENIGN_PATTERNS,
  createConsoleErrorCollector,
} from "./test-helpers";
import {
  resetPerfData,
  capturePerfData,
  assertRenderBudget,
  formatPerfSummary,
} from "./perf-helpers";
import { WEBCAM_PIP_YAML } from "./compositor-fixtures";

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

test.describe("Monitor Session Load Perf — Re-render Budget", () => {
  let collector: ConsoleErrorCollector;
  let sessionId: string | null = null;

  test.beforeEach(async ({ page }) => {
    collector = createConsoleErrorCollector(page);
  });

  test("session load stays within render budget", async ({ page, baseURL }) => {
    // Session creation + full load + settle time.
    test.setTimeout(90_000);

    // ── 1. Create a multi-node session via API ──────────────────────────
    //
    // The Webcam PiP pipeline has ~10 nodes which exercises the session
    // load path well — each node will fire WebSocket state events that
    // our batching should coalesce.

    const apiContext = await request.newContext({
      baseURL: baseURL!,
      extraHTTPHeaders: getAuthHeaders(),
    });

    const sessionName = `perf-monitor-load-${Date.now()}`;
    const createResponse = await apiContext.post("/api/v1/sessions", {
      data: {
        name: sessionName,
        yaml: WEBCAM_PIP_YAML,
      },
    });

    const responseText = await createResponse.text();
    expect(
      createResponse.ok(),
      `Failed to create session: ${responseText}`,
    ).toBeTruthy();

    const createData = JSON.parse(responseText) as { session_id: string };
    sessionId = createData.session_id;
    expect(sessionId).toBeTruthy();
    await apiContext.dispose();

    // ── 2. Navigate to monitor view ─────────────────────────────────────

    await page.goto("/monitor");
    await ensureLoggedIn(page);
    if (!page.url().includes("/monitor")) {
      await page.goto("/monitor");
    }
    await expect(page.getByTestId("monitor-view")).toBeVisible({
      timeout: 15_000,
    });

    // Wait for sessions list to appear.
    await expect(page.getByTestId("sessions-list")).toBeVisible({
      timeout: 10_000,
    });

    // ── 3. Verify dev-mode profiler is available ────────────────────────

    const hasPerfData = await page.evaluate(() => {
      const w = window as Window & {
        __PERF_DATA__?: unknown;
        __PERF_RESET__?: unknown;
      };
      return !!w.__PERF_DATA__ && !!w.__PERF_RESET__;
    });

    if (!hasPerfData) {
      test.skip(
        true,
        "window.__PERF_DATA__ not found — test requires the Vite dev server (just ui)",
      );
    }

    // ── 4. Reset perf data, then click the session ──────────────────────
    //
    // Resetting right before click isolates the session-load renders from
    // any renders that happened during initial page load.

    await resetPerfData(page);

    const sessionItem = page
      .getByTestId("session-item")
      .filter({ hasText: sessionName });
    await expect(sessionItem).toBeVisible({ timeout: 10_000 });
    await sessionItem.click();

    // ── 5. Wait for the session graph to fully load ─────────────────────
    //
    // Wait for React Flow nodes to appear — this signals that the
    // pipeline has been hydrated and rendered.

    await expect(page.locator(".react-flow__node").first()).toBeVisible({
      timeout: 15_000,
    });

    // Allow WebSocket state events to settle. Node state events arrive
    // asynchronously after the pipeline is rendered, so we give them time
    // to be batched and flushed.
    await page.waitForTimeout(2_000);

    // ── 6. Capture and assert render budgets ────────────────────────────

    const snapshot = await capturePerfData(page);
    console.log("\n" + formatPerfSummary(snapshot));

    // MonitorView wraps the entire monitor page including the React Flow
    // canvas, session list sidebar, and all node components.  During a
    // session load without batching, this component would re-render
    // hundreds of times as each WebSocket event triggers a separate
    // Zustand set().  With batching, the render count should be much
    // lower.
    //
    // Budget: 150 renders max during session load.  This is generous
    // enough for CI variance but will catch the pre-optimisation
    // behaviour of 500+ renders for a single session load.
    const monitorData = snapshot.components["MonitorView"];
    expect(
      monitorData,
      `MonitorView profiler data missing. Available components: ${Object.keys(snapshot.components).join(", ") || "(none)"}`,
    ).toBeTruthy();

    if (monitorData) {
      assertRenderBudget(snapshot, "MonitorView", {
        max: 150,
        maxDuration: 3_000,
      });
    }

    // ── 7. Console error check ──────────────────────────────────────────

    const unexpected = collector.getUnexpected(MOQ_BENIGN_PATTERNS);
    if (unexpected.length > 0) {
      console.warn("Unexpected console errors (non-fatal):", unexpected);
    }
  });

  // ── Cleanup ─────────────────────────────────────────────────────────────

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
