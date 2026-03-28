// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * E2E tests for Engine-Native Pipeline Preview (Composable Graph Tap).
 *
 * These tests validate that the preview REST API correctly injects a preview
 * subgraph into a running pipeline, detects audio and video tap points, and
 * tears down cleanly.
 *
 * The tests are API-level: they create sessions via REST, call the preview
 * endpoints, and verify behaviour through the pipeline API — no browser UI
 * interaction is required.
 */

import { test, expect, request } from "@playwright/test";
import * as fs from "fs";
import * as path from "path";

import { getAuthHeaders } from "./auth-helpers";

const FIXTURES_DIR = path.resolve(import.meta.dirname, "../fixtures");

interface NodeInfo {
  kind?: string;
  state?: string | Record<string, unknown>;
  params?: Record<string, unknown>;
  stats?: {
    received?: number;
    sent?: number;
    discarded?: number;
    errored?: number;
  };
}

interface PipelineResponse {
  nodes: Record<string, NodeInfo>;
  connections: Array<{
    from_node: string;
    from_pin: string;
    to_node: string;
    to_pin: string;
    mode?: string;
  }>;
}

interface PreviewResponse {
  preview_id: string;
  gateway_path: string;
  broadcast: string;
  audio: boolean;
  video: boolean;
}

interface PreviewInfo extends PreviewResponse {
  tap_node: string;
  tap_pin: string;
  tap_points: Array<{
    node: string;
    pin: string;
    media: string;
  }>;
  created_at: string;
}

/**
 * Poll the pipeline API until the given predicate returns true, or time out.
 */
async function pollPipeline(
  apiUrl: string,
  sessionId: string,
  predicate: (pipeline: PipelineResponse) => boolean,
  timeoutMs: number = 30_000,
  intervalMs: number = 1_000,
): Promise<PipelineResponse> {
  const deadline = Date.now() + timeoutMs;
  const headers = getAuthHeaders();

  while (Date.now() < deadline) {
    try {
      const response = await fetch(
        `${apiUrl}/api/v1/sessions/${sessionId}/pipeline`,
        { headers },
      );
      if (response.ok) {
        const pipeline = (await response.json()) as PipelineResponse;
        if (predicate(pipeline)) {
          return pipeline;
        }
      }
    } catch {
      // Server not ready or transient error — keep polling
    }
    await new Promise((resolve) => setTimeout(resolve, intervalMs));
  }

  throw new Error(`Pipeline poll timed out after ${timeoutMs}ms`);
}

/** Check whether all nodes in the pipeline have reached the "Running" state. */
function allNodesRunning(pipeline: PipelineResponse): boolean {
  const nodes = Object.values(pipeline.nodes);
  if (nodes.length === 0) return false;
  return nodes.every((n) => n.state === "Running");
}

// ---------------------------------------------------------------------------
// Test: Preview API with a video-only pipeline
// ---------------------------------------------------------------------------

test.describe("Preview API — Video-Only Pipeline", () => {
  let sessionId: string | null = null;

  // A minimal video pipeline: colorbars → pixel_convert → vp9_encoder → sink.
  // No MoQ peer needed — preview will create its own.
  const videoOnlyPipelineYaml = `mode: dynamic
nodes:
  colorbars:
    kind: video::colorbars
    params:
      width: 320
      height: 240
      fps: 10
      pixel_format: rgba8
  pixel_convert:
    kind: video::pixel_convert
    params:
      output_format: i420
    needs: colorbars
  vp9_encoder:
    kind: video::vp9::encoder
    params:
      bitrate_kbps: 500
      cpu_used: 8
      threads: 1
    needs: pixel_convert
  sink:
    kind: core::sink
    needs: vp9_encoder
`;

  test("creates preview, verifies injected nodes, lists it, then stops it", async ({
    baseURL,
  }) => {
    test.setTimeout(90_000);

    const apiContext = await request.newContext({
      baseURL: baseURL!,
      extraHTTPHeaders: getAuthHeaders(),
    });

    try {
      // ── 1. Create session ────────────────────────────────────────────────
      const sessionName = `preview-video-test-${Date.now()}`;
      const createResponse = await apiContext.post("/api/v1/sessions", {
        data: { name: sessionName, yaml: videoOnlyPipelineYaml },
      });
      const createText = await createResponse.text();
      expect(
        createResponse.ok(),
        `Failed to create session: ${createText}`,
      ).toBeTruthy();

      const createData = JSON.parse(createText) as { session_id: string };
      sessionId = createData.session_id;

      // Wait for pipeline nodes to be running
      await pollPipeline(baseURL!, sessionId!, allNodesRunning, 30_000);

      // ── 2. Start preview (auto-detect tap point) ─────────────────────────
      const previewResponse = await apiContext.post(
        `/api/v1/sessions/${sessionId}/preview`,
        { data: {} },
      );
      const previewText = await previewResponse.text();
      expect(
        previewResponse.status(),
        `Expected 201 Created, got ${previewResponse.status()}: ${previewText}`,
      ).toBe(201);

      const preview = JSON.parse(previewText) as PreviewResponse;
      expect(preview.preview_id).toBeTruthy();
      expect(preview.gateway_path).toContain("/_preview/");
      expect(preview.broadcast).toBe("output");
      // Video-only pipeline should detect video
      expect(preview.video).toBe(true);

      // ── 3. Verify the preview subgraph was injected ──────────────────────
      // Wait for the preview moq_peer node to appear in the pipeline
      const pipeline = await pollPipeline(
        baseURL!,
        sessionId!,
        (p) => {
          const previewPeerId = `_preview_${preview.preview_id}_peer`;
          return previewPeerId in p.nodes;
        },
        15_000,
      );

      const previewPeerId = `_preview_${preview.preview_id}_peer`;
      expect(pipeline.nodes[previewPeerId]).toBeDefined();
      expect(pipeline.nodes[previewPeerId].kind).toBe("transport::moq::peer");

      // Verify there's a BestEffort connection into the preview subgraph
      const previewConnections = pipeline.connections.filter(
        (c) =>
          c.to_node.startsWith(`_preview_${preview.preview_id}_`) ||
          c.from_node.startsWith(`_preview_${preview.preview_id}_`),
      );
      expect(previewConnections.length).toBeGreaterThan(0);

      // The tap connection (from the existing pipeline into the preview)
      // should use BestEffort mode
      const tapConnection = previewConnections.find(
        (c) => !c.from_node.startsWith(`_preview_${preview.preview_id}_`),
      );
      expect(tapConnection).toBeDefined();
      expect(tapConnection!.mode).toBe("BestEffort");

      // ── 4. List previews ─────────────────────────────────────────────────
      const listResponse = await apiContext.get(
        `/api/v1/sessions/${sessionId}/preview`,
      );
      expect(listResponse.ok()).toBeTruthy();
      const previews = (await listResponse.json()) as PreviewInfo[];
      expect(previews.length).toBe(1);
      expect(previews[0].preview_id).toBe(preview.preview_id);

      // ── 5. Stop preview ──────────────────────────────────────────────────
      const stopResponse = await apiContext.delete(
        `/api/v1/sessions/${sessionId}/preview/${preview.preview_id}`,
      );
      expect(stopResponse.ok()).toBeTruthy();

      // Verify the preview nodes are removed from the pipeline
      const cleanedPipeline = await pollPipeline(
        baseURL!,
        sessionId!,
        (p) => !(previewPeerId in p.nodes),
        15_000,
      );
      expect(cleanedPipeline.nodes[previewPeerId]).toBeUndefined();

      // ── 6. Verify no previews remain ─────────────────────────────────────
      const listAfterStop = await apiContext.get(
        `/api/v1/sessions/${sessionId}/preview`,
      );
      const previewsAfterStop = (await listAfterStop.json()) as PreviewInfo[];
      expect(previewsAfterStop.length).toBe(0);
    } finally {
      await apiContext.dispose();
    }
  });

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
        // Best-effort cleanup
      }
      sessionId = null;
    }
  });
});

// ---------------------------------------------------------------------------
// Test: Preview API with audio + video pipeline (MoQ peer pipeline)
// ---------------------------------------------------------------------------

test.describe("Preview API — Audio + Video Pipeline", () => {
  let sessionId: string | null = null;

  test("auto-detects both audio and video tap points in a full MoQ pipeline", async ({
    baseURL,
  }) => {
    test.setTimeout(120_000);

    // Use the compositor-colorbars fixture — it has video but no audio
    // encoding chain. We'll create a pipeline with both.
    const fullPipelineYaml = fs.readFileSync(
      path.join(FIXTURES_DIR, "compositor-colorbars.yaml"),
      "utf8",
    );

    const apiContext = await request.newContext({
      baseURL: baseURL!,
      extraHTTPHeaders: getAuthHeaders(),
    });

    try {
      // ── 1. Create session ────────────────────────────────────────────────
      const sessionName = `preview-av-test-${Date.now()}`;
      const createResponse = await apiContext.post("/api/v1/sessions", {
        data: { name: sessionName, yaml: fullPipelineYaml },
      });
      const createText = await createResponse.text();
      expect(
        createResponse.ok(),
        `Failed to create session: ${createText}`,
      ).toBeTruthy();

      const createData = JSON.parse(createText) as { session_id: string };
      sessionId = createData.session_id;

      // Wait for pipeline to be running
      await pollPipeline(baseURL!, sessionId!, allNodesRunning, 60_000);

      // ── 2. Start preview ─────────────────────────────────────────────────
      const previewResponse = await apiContext.post(
        `/api/v1/sessions/${sessionId}/preview`,
        { data: {} },
      );
      const previewText = await previewResponse.text();
      expect(
        previewResponse.status(),
        `Expected 201 Created, got ${previewResponse.status()}: ${previewText}`,
      ).toBe(201);

      const preview = JSON.parse(previewText) as PreviewResponse;
      expect(preview.preview_id).toBeTruthy();
      // This pipeline has video only (no audio encoder chain feeding moq_peer)
      expect(preview.video).toBe(true);

      // ── 3. Verify preview nodes appear ───────────────────────────────────
      const previewPeerId = `_preview_${preview.preview_id}_peer`;
      await pollPipeline(
        baseURL!,
        sessionId!,
        (p) => previewPeerId in p.nodes,
        15_000,
      );

      // ── 4. Stop preview and verify cleanup ───────────────────────────────
      const stopResponse = await apiContext.delete(
        `/api/v1/sessions/${sessionId}/preview/${preview.preview_id}`,
      );
      expect(stopResponse.ok()).toBeTruthy();

      await pollPipeline(
        baseURL!,
        sessionId!,
        (p) => !(previewPeerId in p.nodes),
        15_000,
      );
    } finally {
      await apiContext.dispose();
    }
  });

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
        // Best-effort cleanup
      }
      sessionId = null;
    }
  });
});

// ---------------------------------------------------------------------------
// Test: Preview API error cases
// ---------------------------------------------------------------------------

test.describe("Preview API — Error Cases", () => {
  let sessionId: string | null = null;

  const minimalPipelineYaml = `mode: dynamic
nodes:
  colorbars:
    kind: video::colorbars
    params:
      width: 320
      height: 240
      fps: 10
  sink:
    kind: core::sink
    needs: colorbars
`;

  test("returns 404 for non-existent session", async ({ baseURL }) => {
    const apiContext = await request.newContext({
      baseURL: baseURL!,
      extraHTTPHeaders: getAuthHeaders(),
    });

    try {
      const response = await apiContext.post(
        "/api/v1/sessions/nonexistent-session-id/preview",
        { data: {} },
      );
      expect(response.status()).toBe(404);
    } finally {
      await apiContext.dispose();
    }
  });

  test("returns 400 for non-existent tap node", async ({ baseURL }) => {
    const apiContext = await request.newContext({
      baseURL: baseURL!,
      extraHTTPHeaders: getAuthHeaders(),
    });

    try {
      const sessionName = `preview-error-test-${Date.now()}`;
      const createResponse = await apiContext.post("/api/v1/sessions", {
        data: { name: sessionName, yaml: minimalPipelineYaml },
      });
      expect(createResponse.ok()).toBeTruthy();
      const createData = (await createResponse.json()) as {
        session_id: string;
      };
      sessionId = createData.session_id;

      await pollPipeline(baseURL!, sessionId!, allNodesRunning, 30_000);

      const response = await apiContext.post(
        `/api/v1/sessions/${sessionId}/preview`,
        { data: { tap_node: "nonexistent_node", tap_pin: "out" } },
      );
      expect(response.status()).toBe(400);
    } finally {
      await apiContext.dispose();
    }
  });

  test("returns 409 when preview limit is exceeded", async ({ baseURL }) => {
    test.setTimeout(90_000);

    const apiContext = await request.newContext({
      baseURL: baseURL!,
      extraHTTPHeaders: getAuthHeaders(),
    });

    try {
      const sessionName = `preview-limit-test-${Date.now()}`;
      const createResponse = await apiContext.post("/api/v1/sessions", {
        data: { name: sessionName, yaml: minimalPipelineYaml },
      });
      expect(createResponse.ok()).toBeTruthy();
      const createData = (await createResponse.json()) as {
        session_id: string;
      };
      sessionId = createData.session_id;

      await pollPipeline(baseURL!, sessionId!, allNodesRunning, 30_000);

      // Create first preview — should succeed
      const preview1 = await apiContext.post(
        `/api/v1/sessions/${sessionId}/preview`,
        { data: {} },
      );
      expect(preview1.status()).toBe(201);

      // Create second preview — should succeed
      const preview2 = await apiContext.post(
        `/api/v1/sessions/${sessionId}/preview`,
        { data: {} },
      );
      expect(preview2.status()).toBe(201);

      // Create third preview — should fail with 409
      const preview3 = await apiContext.post(
        `/api/v1/sessions/${sessionId}/preview`,
        { data: {} },
      );
      expect(preview3.status()).toBe(409);
    } finally {
      await apiContext.dispose();
    }
  });

  test("returns 404 when stopping non-existent preview", async ({
    baseURL,
  }) => {
    const apiContext = await request.newContext({
      baseURL: baseURL!,
      extraHTTPHeaders: getAuthHeaders(),
    });

    try {
      const sessionName = `preview-stop-404-test-${Date.now()}`;
      const createResponse = await apiContext.post("/api/v1/sessions", {
        data: { name: sessionName, yaml: minimalPipelineYaml },
      });
      expect(createResponse.ok()).toBeTruthy();
      const createData = (await createResponse.json()) as {
        session_id: string;
      };
      sessionId = createData.session_id;

      const response = await apiContext.delete(
        `/api/v1/sessions/${sessionId}/preview/nonexistent-preview-id`,
      );
      expect(response.status()).toBe(404);
    } finally {
      await apiContext.dispose();
    }
  });

  test("session destroy cleans up active previews", async ({ baseURL }) => {
    test.setTimeout(60_000);

    const apiContext = await request.newContext({
      baseURL: baseURL!,
      extraHTTPHeaders: getAuthHeaders(),
    });

    try {
      const sessionName = `preview-cleanup-test-${Date.now()}`;
      const createResponse = await apiContext.post("/api/v1/sessions", {
        data: { name: sessionName, yaml: minimalPipelineYaml },
      });
      expect(createResponse.ok()).toBeTruthy();
      const createData = (await createResponse.json()) as {
        session_id: string;
      };
      sessionId = createData.session_id;

      await pollPipeline(baseURL!, sessionId!, allNodesRunning, 30_000);

      // Start a preview
      const previewResponse = await apiContext.post(
        `/api/v1/sessions/${sessionId}/preview`,
        { data: {} },
      );
      expect(previewResponse.status()).toBe(201);

      // Delete the session — should clean up preview automatically
      const deleteResponse = await apiContext.delete(
        `/api/v1/sessions/${sessionId}`,
      );
      expect(deleteResponse.ok()).toBeTruthy();
      sessionId = null; // Already deleted

      // Verify session is gone
      const listResponse = await apiContext.get("/api/v1/sessions");
      const sessions = (await listResponse.json()) as Array<{ id: string }>;
      const found = sessions.find((s) => s.id === createData.session_id);
      expect(found).toBeUndefined();
    } finally {
      await apiContext.dispose();
    }
  });

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
        // Best-effort cleanup
      }
      sessionId = null;
    }
  });
});
