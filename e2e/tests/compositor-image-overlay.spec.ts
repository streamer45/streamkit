// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * E2E tests for compositor image overlay lifecycle.
 *
 * Creates a Webcam PiP pipeline session via the API, navigates to the
 * monitor view where the compositor node graph is rendered, then exercises
 * the image overlay workflow:
 *
 * - Add menu shows "Text" and "Image" options
 * - Adding an image via file picker creates a new layer
 * - Image layer appears in the layer list and on the canvas
 * - Selecting image layer shows inspector controls (opacity, rotation, mirror)
 * - Crop & Zoom section is NOT shown for image layers
 * - Image overlay can be deleted via context menu
 */

import { test, expect, request, type Page } from "@playwright/test";

import { ensureLoggedIn, getAuthHeaders } from "./auth-helpers";
import {
  type ConsoleErrorCollector,
  MOQ_BENIGN_PATTERNS,
  createConsoleErrorCollector,
} from "./test-helpers";
import { WEBCAM_PIP_YAML } from "./compositor-fixtures";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async function setupCompositorView(page: Page) {
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
  const sessionItem = page.getByTestId("session-item").first();
  await expect(sessionItem).toBeVisible({ timeout: 10_000 });
  await sessionItem.click();

  await expect(page.locator(".react-flow__node").first()).toBeVisible({
    timeout: 15_000,
  });

  const compositorNode = page
    .locator(".react-flow__node")
    .filter({ hasText: "Compositor" });
  await expect(compositorNode).toBeVisible({ timeout: 10_000 });

  const canvasInner = compositorNode.locator("[data-canvas-width]");
  await expect(canvasInner).toBeVisible({ timeout: 5_000 });

  return { compositorNode, canvasInner };
}

/**
 * Generate a valid 200×200 solid-red PNG buffer.
 *
 * Using a reasonably-sized image ensures the compositor creates a canvas
 * overlay box large enough to be visible and clickable during tests.
 * (A 1×1 PNG would produce a 1×1-pixel layer box that Playwright can't
 * reliably interact with.)
 */
function createTestPngBuffer(): Buffer {
  const base64 =
    "iVBORw0KGgoAAAANSUhEUgAAAMgAAADICAIAAAAiOjnJAAACcklEQVR4nO3OAQkAMBDE" +
    "sPNvejPxUCiFCMjelpzjB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1Hi" +
    "B1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1Hi" +
    "B1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1Hi" +
    "B1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1Hi" +
    "B1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1Hi" +
    "B1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1Hi" +
    "B1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1Hi" +
    "B1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1Hi" +
    "B1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1Hi" +
    "B1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1Hi" +
    "B1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1HiB1Hi" +
    "B1HiB1HiB1HiB1HiB1HiB1HiB1H60Yes1qIoPaoAAAAASUVORK5CYII=";
  return Buffer.from(base64, "base64");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

test.describe("Compositor Image Overlay Lifecycle", () => {
  let collector: ConsoleErrorCollector;
  let sessionId: string | null = null;

  test.beforeEach(async ({ page }) => {
    collector = createConsoleErrorCollector(page);
  });

  test("add image overlay, inspect controls, delete via context menu", async ({
    page,
    baseURL,
  }) => {
    test.setTimeout(120_000);

    // ── 1. Create session via API ────────────────────────────────────────

    const apiContext = await request.newContext({
      baseURL: baseURL!,
      extraHTTPHeaders: getAuthHeaders(),
    });

    const createResponse = await apiContext.post("/api/v1/sessions", {
      data: {
        name: `image-overlay-test-${Date.now()}`,
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

    // ── 2. Navigate to monitor view, find compositor node ────────────────

    const { compositorNode, canvasInner } = await setupCompositorView(page);

    // Verify baseline layers exist.
    await expect(
      compositorNode.getByText("Input 0", { exact: true }).first(),
    ).toBeVisible({
      timeout: 5_000,
    });
    await expect(
      compositorNode.getByText("Text 0", { exact: true }).first(),
    ).toBeVisible({
      timeout: 5_000,
    });

    // ── 3. Open Add menu — verify Text and Image options ─────────────────

    const addButton = compositorNode.getByRole("button", { name: /Add/i });
    await expect(addButton).toBeVisible({ timeout: 5_000 });
    await addButton.click();

    // The add menu should show "Text" and "Image" options.
    const textMenuItem = compositorNode
      .getByText("Text", { exact: true })
      .last();
    const imageMenuItem = compositorNode
      .getByText("Image", { exact: true })
      .last();
    await expect(textMenuItem).toBeVisible({ timeout: 3_000 });
    await expect(imageMenuItem).toBeVisible({ timeout: 3_000 });

    // ── 4. Add image overlay via file picker ─────────────────────────────

    // The "Image" menu item triggers a hidden file input via a programmatic
    // click.  We use Playwright's filechooser event to intercept the native
    // file dialog and supply a test PNG.
    const [fileChooser] = await Promise.all([
      page.waitForEvent("filechooser"),
      imageMenuItem.click(),
    ]);

    await fileChooser.setFiles({
      name: "test-image.png",
      mimeType: "image/png",
      buffer: createTestPngBuffer(),
    });

    // ── 5. Verify image layer appears in layer list ──────────────────────

    // Wait for the image layer to appear.  The layer list and the canvas
    // both render "Image 0" text.  We target the layer list item which is
    // NOT inside the canvas [data-canvas-width] area.
    const imageLayer = compositorNode
      .getByText("Image 0", { exact: true })
      .first();
    await expect(imageLayer).toBeVisible({ timeout: 5_000 });

    // ── 6. Verify image layer box appears on canvas ──────────────────────

    // The image overlay box on the canvas has a unique aria-label.
    const imageLayerBox = canvasInner
      .locator('[aria-label*="Image overlay"]')
      .first();
    await expect(imageLayerBox).toBeVisible({ timeout: 5_000 });

    // ── 7. Select image layer — inspector controls appear ────────────────

    // Click the image layer box on the canvas to select it.  The canvas box
    // has a unique aria-label so we avoid ambiguity with the layer list text.
    await imageLayerBox.click();

    // Wait for the inspector panel to render (slider becomes visible).
    await expect(compositorNode.getByRole("slider").first()).toBeVisible({
      timeout: 5_000,
    });

    // Opacity section: contains the "Opacity" label and a slider.
    const opacitySection = compositorNode
      .locator("div")
      .filter({ hasText: /^Opacity/ })
      .filter({ has: page.getByRole("slider") })
      .first();
    await expect(opacitySection).toBeVisible({ timeout: 5_000 });

    // Rotation section should be visible.
    const rotationSection = compositorNode
      .locator("div")
      .filter({ hasText: /^Rotation/ })
      .first();
    await expect(rotationSection).toBeVisible({ timeout: 5_000 });

    // Mirror section should be visible.
    const mirrorSection = compositorNode
      .locator("div")
      .filter({ hasText: /^Mirror/ })
      .first();
    await expect(mirrorSection).toBeVisible({ timeout: 5_000 });

    // ── 8. Crop & Zoom should NOT appear for image layers ────────────────

    const cropSection = compositorNode.getByTestId("crop-zoom-section");
    await expect(cropSection).not.toBeVisible({ timeout: 3_000 });

    // ── 9. Delete image overlay via context menu ─────────────────────────

    // Right-click the image layer box on the canvas to open context menu.
    await imageLayerBox.click({ button: "right" });

    const contextMenu = page.getByTestId("compositor-context-menu");
    await expect(contextMenu).toBeVisible({ timeout: 3_000 });

    // Image overlays should have a "Delete" option.
    const deleteOption = page.getByTestId("ctx-delete");
    await expect(deleteOption).toBeVisible();
    await deleteOption.click();

    // "Image 0" should be removed from the layer list.
    await expect(
      compositorNode.getByText("Image 0", { exact: true }).first(),
    ).not.toBeVisible({
      timeout: 5_000,
    });

    // ── 10. Console error check ──────────────────────────────────────────

    const unexpected = collector.getUnexpected(MOQ_BENIGN_PATTERNS);
    if (unexpected.length > 0) {
      console.warn("Unexpected console errors (non-fatal):", unexpected);
    }
  });

  test("image asset upload happy path: upload, list, serve, duplicate 409, delete", async ({
    baseURL,
  }) => {
    test.setTimeout(60_000);

    const apiContext = await request.newContext({
      baseURL: baseURL!,
      extraHTTPHeaders: getAuthHeaders(),
    });

    const testFileName = `e2e-test-${Date.now()}.png`;
    const pngBuffer = createTestPngBuffer();

    // ── 1. Upload image asset via multipart POST ─────────────────────────

    const uploadResponse = await apiContext.post("/api/v1/assets/images", {
      multipart: {
        file: {
          name: testFileName,
          mimeType: "image/png",
          buffer: pngBuffer,
        },
      },
    });

    expect(
      uploadResponse.ok(),
      `Upload failed: ${await uploadResponse.text()}`,
    ).toBeTruthy();

    const asset = (await uploadResponse.json()) as {
      id: string;
      name: string;
      path: string;
      format: string;
      width: number;
      height: number;
      size_bytes: number;
      is_system: boolean;
    };

    expect(asset.id).toBe(testFileName);
    expect(asset.name).toBe(testFileName);
    expect(asset.path).toContain("samples/images/user/");
    expect(asset.format).toBe("png");
    expect(asset.width).toBe(200);
    expect(asset.height).toBe(200);
    expect(asset.size_bytes).toBeGreaterThan(0);
    expect(asset.is_system).toBe(false);

    // ── 2. List assets — uploaded image should appear ─────────────────────

    const listResponse = await apiContext.get("/api/v1/assets/images");
    expect(listResponse.ok()).toBeTruthy();

    const assets = (await listResponse.json()) as Array<{
      id: string;
      path: string;
    }>;
    const found = assets.find((a) => a.id === testFileName);
    expect(
      found,
      `Uploaded asset ${testFileName} not found in list`,
    ).toBeTruthy();

    // ── 3. Serve image file — GET /api/v1/assets/images/file/{id} ────────

    const serveResponse = await apiContext.get(
      `/api/v1/assets/images/file/${encodeURIComponent(testFileName)}`,
    );
    expect(
      serveResponse.ok(),
      `Serve failed: ${serveResponse.status()}`,
    ).toBeTruthy();

    const contentType = serveResponse.headers()["content-type"] ?? "";
    expect(contentType).toContain("image/png");

    const servedBytes = await serveResponse.body();
    expect(servedBytes.length).toBe(pngBuffer.length);

    // ── 4. Duplicate upload — expect 409 Conflict ────────────────────────

    const dupResponse = await apiContext.post("/api/v1/assets/images", {
      multipart: {
        file: {
          name: testFileName,
          mimeType: "image/png",
          buffer: pngBuffer,
        },
      },
    });

    expect(dupResponse.status()).toBe(409);

    // ── 5. Delete asset ──────────────────────────────────────────────────

    const deleteResponse = await apiContext.delete(
      `/api/v1/assets/images/${encodeURIComponent(testFileName)}`,
    );
    expect(
      deleteResponse.ok(),
      `Delete failed: ${deleteResponse.status()}`,
    ).toBeTruthy();

    // ── 6. Verify asset is gone from list ────────────────────────────────

    const listAfterDelete = await apiContext.get("/api/v1/assets/images");
    expect(listAfterDelete.ok()).toBeTruthy();

    const assetsAfter = (await listAfterDelete.json()) as Array<{ id: string }>;
    const stillExists = assetsAfter.find((a) => a.id === testFileName);
    expect(stillExists, `Asset ${testFileName} should be deleted`).toBeFalsy();

    await apiContext.dispose();
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
