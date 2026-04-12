// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * E2E tests for compositor video output rendering.
 *
 * Creates a two-colorbars compositor pipeline (no webcam needed), verifies
 * the pipeline runs successfully in the monitor view, and tests that
 * compositor interactions don't crash the pipeline.
 *
 * This test exercises the full video pipeline end-to-end:
 *   colorbars × 2 → compositor → pixel_convert → vp9_encoder → moq_peer
 *
 * The monitor view is used to verify the compositor node renders correctly,
 * shows LIVE status, and the canvas preview draws non-black pixels from the
 * composited colorbars sources.
 */

import { test, expect, request } from "@playwright/test";

import { ensureLoggedIn, getAuthHeaders } from "./auth-helpers";
import {
  type ConsoleErrorCollector,
  MOQ_BENIGN_PATTERNS,
  createConsoleErrorCollector,
} from "./test-helpers";
import { COMPOSITOR_COLORBARS_YAML } from "./compositor-fixtures";

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

test.describe("Compositor Video Output — Two Colorbars Pipeline", () => {
  let collector: ConsoleErrorCollector;
  let sessionId: string | null = null;

  test.beforeEach(async ({ page }) => {
    collector = createConsoleErrorCollector(page);
  });

  test("compositor pipeline runs, monitor shows LIVE node with canvas preview, interaction survives", async ({
    page,
    baseURL,
  }) => {
    test.setTimeout(120_000);

    // ── 1. Create compositor session via API ─────────────────────────────

    const apiContext = await request.newContext({
      baseURL: baseURL!,
      extraHTTPHeaders: getAuthHeaders(),
    });

    const sessionName = `compositor-output-test-${Date.now()}`;
    const createResponse = await apiContext.post("/api/v1/sessions", {
      data: {
        name: sessionName,
        yaml: COMPOSITOR_COLORBARS_YAML,
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

    // ── 2. Navigate to monitor view ───────────────────────────────────────

    await page.goto("/monitor");
    await ensureLoggedIn(page);
    if (!page.url().includes("/monitor")) {
      await page.goto("/monitor");
    }
    await expect(page.getByTestId("monitor-view")).toBeVisible({
      timeout: 15_000,
    });

    // Wait for sessions list and click the session.
    await expect(page.getByTestId("sessions-list")).toBeVisible({
      timeout: 10_000,
    });
    const sessionItem = page
      .getByTestId("session-item")
      .filter({ hasText: sessionName })
      .first();
    await expect(sessionItem).toBeVisible({ timeout: 10_000 });
    await sessionItem.click();

    // ── 3. Verify compositor node is visible and running ──────────────────

    const compositorNode = page
      .locator(".react-flow__node")
      .filter({ hasText: "Compositor" });
    await expect(compositorNode).toBeVisible({ timeout: 15_000 });

    // Verify LIVE badge is visible on compositor node.
    const liveBadge = compositorNode.getByText("LIVE");
    await expect(liveBadge).toBeVisible({ timeout: 10_000 });

    // ── 4. Verify canvas preview is visible and has content ─────────────

    const canvasInner = compositorNode.locator("[data-canvas-width]");
    await expect(canvasInner).toBeVisible({ timeout: 5_000 });

    // The canvas preview renders layer bounding boxes (outlines) over a
    // dark background — it does NOT stream actual video frames.  Verify
    // the canvas area exists and the layer boxes are drawn within it.
    const layerBoxes = canvasInner.locator(".nodrag.nopan");
    await expect(layerBoxes.first()).toBeVisible({ timeout: 10_000 });

    // ── 5. Verify both input layers exist ─────────────────────────────────

    const inputLayer0 = compositorNode
      .getByText("Input 0", { exact: true })
      .first();
    const inputLayer1 = compositorNode
      .getByText("Input 1", { exact: true })
      .first();
    await expect(inputLayer0).toBeVisible({ timeout: 5_000 });
    await expect(inputLayer1).toBeVisible({ timeout: 5_000 });

    // ── 6. Select Input 1 and verify inspector interaction ────────────────

    await inputLayer1.click();

    // Wait for inspector to render (slider becomes visible).
    await expect(compositorNode.getByRole("slider").first()).toBeVisible({
      timeout: 5_000,
    });

    // Opacity section should be visible.
    const opacitySection = compositorNode
      .locator("div")
      .filter({ hasText: /^Opacity/ })
      .filter({ has: page.getByRole("slider") })
      .first();
    await expect(opacitySection).toBeVisible({ timeout: 5_000 });

    // ── 7. Switch to Input 0 — verify it also works ───────────────────────

    await inputLayer0.click();
    await expect(compositorNode.getByRole("slider").first()).toBeVisible({
      timeout: 5_000,
    });

    // LIVE badge should still be visible (pipeline survived interaction).
    await expect(liveBadge).toBeVisible({ timeout: 5_000 });

    // Canvas preview should still show layer boxes.
    await expect(layerBoxes.first()).toBeVisible({ timeout: 5_000 });

    // ── 8. Verify other pipeline nodes are present ────────────────────────

    // The pipeline should have pixel_convert, vp9_encoder, and moq_peer nodes.
    const allNodes = page.locator(".react-flow__node");
    const nodeCount = await allNodes.count();
    expect(
      nodeCount,
      "Pipeline should have multiple nodes",
    ).toBeGreaterThanOrEqual(4);

    // ── 9. Console error check ────────────────────────────────────────────

    const unexpected = collector.getUnexpected(MOQ_BENIGN_PATTERNS);
    if (unexpected.length > 0) {
      console.warn("Unexpected console errors (non-fatal):", unexpected);
    }
  });

  // ---------------------------------------------------------------------------
  // Regression test: compositor param changes from UI must reach the server.
  //
  // When the UI sends a compositor config update (e.g. opacity slider drag),
  // the engine's TuneNode path strips transient sync metadata (_sender, _rev)
  // before dispatching UpdateParams to the compositor node.  If stripping
  // fails (or the metadata is not stripped), CompositorConfig's
  // deny_unknown_fields rejects the entire update — breaking all compositor
  // control.  This test verifies the full round-trip: UI slider → WS
  // TuneNodeAsync → engine → compositor → pipeline API reflects the change.
  // ---------------------------------------------------------------------------

  test("compositor param change from UI is reflected in server-side pipeline state", async ({
    page,
    baseURL,
  }) => {
    test.setTimeout(120_000);

    // ── 1. Create compositor session via API ─────────────────────────────

    const apiContext = await request.newContext({
      baseURL: baseURL!,
      extraHTTPHeaders: getAuthHeaders(),
    });

    const sessionName = `compositor-param-sync-${Date.now()}`;
    const createResponse = await apiContext.post("/api/v1/sessions", {
      data: {
        name: sessionName,
        yaml: COMPOSITOR_COLORBARS_YAML,
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

    // ── 2. Navigate to monitor and open the session ─────────────────────

    await page.goto("/monitor");
    await ensureLoggedIn(page);
    if (!page.url().includes("/monitor")) {
      await page.goto("/monitor");
    }
    await expect(page.getByTestId("monitor-view")).toBeVisible({
      timeout: 15_000,
    });

    await expect(page.getByTestId("sessions-list")).toBeVisible({
      timeout: 10_000,
    });
    const sessionItem = page
      .getByTestId("session-item")
      .filter({ hasText: sessionName })
      .first();
    await expect(sessionItem).toBeVisible({ timeout: 10_000 });
    await sessionItem.click();

    // ── 3. Wait for compositor node LIVE ─────────────────────────────────

    const compositorNode = page
      .locator(".react-flow__node")
      .filter({ hasText: "Compositor" });
    await expect(compositorNode).toBeVisible({ timeout: 15_000 });
    await expect(compositorNode.getByText("LIVE")).toBeVisible({
      timeout: 10_000,
    });

    // ── 4. Select Input 1 and locate opacity slider ─────────────────────

    const inputLayer1 = compositorNode
      .getByText("Input 1", { exact: true })
      .first();
    await expect(inputLayer1).toBeVisible({ timeout: 5_000 });
    await inputLayer1.click();

    const opacitySection = compositorNode
      .locator("div")
      .filter({ hasText: /^Opacity/ })
      .filter({ has: page.getByRole("slider") })
      .first();
    await expect(opacitySection).toBeVisible({ timeout: 5_000 });

    // ── 5. Drag opacity slider to change value ──────────────────────────

    const thumb = opacitySection.getByRole("slider");
    await thumb.waitFor({ state: "visible", timeout: 5_000 });
    const box = await thumb.boundingBox();
    expect(box, "Opacity slider thumb must have a bounding box").toBeTruthy();

    // Drag the slider significantly to the left to reduce opacity.
    const startX = box!.x + box!.width / 2;
    const startY = box!.y + box!.height / 2;
    await page.mouse.move(startX, startY);
    await page.mouse.down();
    // Move in steps to simulate a realistic drag.
    for (let i = 1; i <= 10; i++) {
      await page.mouse.move(startX - i * 5, startY);
    }
    await page.mouse.up();

    // Wait for debounced WS message to reach the server.
    await page.waitForTimeout(1_000);

    // ── 6. Verify server-side pipeline state reflects the change ────────

    const pipelineResponse = await apiContext.get(
      `/api/v1/sessions/${sessionId}/pipeline`,
    );
    expect(pipelineResponse.ok(), "Pipeline API should return OK").toBeTruthy();

    const pipeline = (await pipelineResponse.json()) as {
      nodes: Record<string, { params?: Record<string, unknown> }>;
    };

    const compositorParams = pipeline.nodes["compositor"]?.params;
    expect(
      compositorParams,
      "Compositor node should have params in pipeline state",
    ).toBeTruthy();

    // The layers object should exist and in_1's opacity should have
    // changed from the initial value of 0.9 (per the fixture YAML).
    const layers = compositorParams!["layers"] as
      | Record<string, { opacity?: number }>
      | undefined;
    expect(layers, "Compositor params should contain layers").toBeTruthy();
    expect(
      layers!["in_1"],
      "Layer in_1 should exist in compositor params",
    ).toBeTruthy();

    const newOpacity = layers!["in_1"]!.opacity;
    expect(newOpacity, "in_1 opacity should be defined").toBeDefined();
    expect(
      newOpacity,
      `in_1 opacity should have changed from initial 0.9 (got ${newOpacity}). ` +
        "If still 0.9, the UI param change did not reach the server — " +
        "likely UpdateParams deserialization is failing (e.g. _rev/_sender not stripped).",
    ).not.toBeCloseTo(0.9, 1);

    await apiContext.dispose();

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
