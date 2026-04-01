// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * E2E tests for AV1 decode → compositor → SVT-AV1 encode relay pipeline.
 *
 * These tests validate the full AV1 video processing path:
 *   colorbars → AV1 encode (rav1e) → relay → AV1 decode (rav1d) →
 *   compositor → pixel_convert → SVT-AV1 encode → relay
 *
 * The pipeline is self-contained (no browser or webcam needed) and exercises
 * both the AV1 decoder and SVT-AV1 encoder nodes through a MoQ relay.
 *
 * Prerequisites:
 * - skit built with --features svt_av1
 * - A moq-relay binary (built by `just build-moq-relay` or set via `E2E_MOQ_RELAY_BIN`)
 * - The test harness starts moq-relay automatically and exposes `E2E_RELAY_URL`
 *
 * If the relay is not available, tests skip gracefully.
 */

import { test, expect, request } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';

import { getAuthHeaders } from './auth-helpers';

const FIXTURES_DIR = path.resolve(import.meta.dirname, '../fixtures');

/** Read a pipeline fixture and substitute the relay URL placeholder. */
function loadPipelineFixture(filename: string, relayUrl: string): string {
  const raw = fs.readFileSync(path.join(FIXTURES_DIR, filename), 'utf8');
  return raw.replaceAll('${RELAY_URL}', relayUrl);
}

interface NodeInfo {
  kind?: string;
  state?: string | Record<string, unknown>;
  stats?: {
    received?: number;
    sent?: number;
    discarded?: number;
    errored?: number;
  };
}

interface PipelineResponse {
  nodes: Record<string, NodeInfo>;
}

/**
 * Poll the pipeline API until the given predicate returns true, or time out.
 */
async function pollPipeline(
  apiUrl: string,
  sessionId: string,
  predicate: (pipeline: PipelineResponse) => boolean,
  timeoutMs: number = 30_000,
  intervalMs: number = 1_000
): Promise<PipelineResponse> {
  const deadline = Date.now() + timeoutMs;
  const headers = getAuthHeaders();

  while (Date.now() < deadline) {
    try {
      const response = await fetch(`${apiUrl}/api/v1/sessions/${sessionId}/pipeline`, { headers });
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

  // One last attempt to get the current state for the error message
  try {
    const response = await fetch(`${apiUrl}/api/v1/sessions/${sessionId}/pipeline`, { headers });
    if (response.ok) {
      const pipeline = (await response.json()) as PipelineResponse;
      const states = Object.entries(pipeline.nodes)
        .map(([id, n]) => `${id}: ${JSON.stringify(n.state) ?? 'unknown'}`)
        .join(', ');
      throw new Error(`Pipeline poll timed out after ${timeoutMs}ms. Node states: ${states}`);
    }
  } catch (e) {
    if (e instanceof Error && e.message.startsWith('Pipeline poll timed out')) throw e;
  }

  throw new Error(`Pipeline poll timed out after ${timeoutMs}ms (could not fetch final state)`);
}

/** Check whether all nodes in the pipeline have reached the "Running" state. */
function allNodesRunning(pipeline: PipelineResponse): boolean {
  const nodes = Object.values(pipeline.nodes);
  if (nodes.length === 0) return false;
  return nodes.every((n) => n.state === 'Running');
}

/** Check whether no nodes have entered a "Failed" state. */
function noNodesFailed(pipeline: PipelineResponse): boolean {
  return Object.values(pipeline.nodes).every(
    (n) => typeof n.state !== 'object' || !('Failed' in (n.state as object))
  );
}

// ---------------------------------------------------------------------------
// Test: AV1 decode → compositor → SVT-AV1 encode (self-contained via relay)
// ---------------------------------------------------------------------------

test.describe('MoQ Relay — AV1 Compositor Pipeline (SVT-AV1)', () => {
  let sessionId: string | null = null;

  test('creates AV1 compositor pipeline, verifies AV1 decode and SVT-AV1 encode nodes reach running state', async ({
    baseURL,
  }) => {
    test.setTimeout(180_000);

    const relayUrl = process.env.E2E_RELAY_URL;
    if (!relayUrl) {
      test.skip(true, 'moq-relay not available (E2E_RELAY_URL not set)');
      return;
    }

    // ── 1. Create session via API ──────────────────────────────────────────

    const yaml = loadPipelineFixture('moq-relay-av1-compositor.yaml', relayUrl);

    const apiContext = await request.newContext({
      baseURL: baseURL!,
      extraHTTPHeaders: getAuthHeaders(),
    });

    const sessionName = `relay-av1-compositor-test-${Date.now()}`;
    const createResponse = await apiContext.post('/api/v1/sessions', {
      data: { name: sessionName, yaml },
    });

    const responseText = await createResponse.text();
    expect(createResponse.ok(), `Failed to create session: ${responseText}`).toBeTruthy();

    const createData = JSON.parse(responseText) as { session_id: string };
    sessionId = createData.session_id;
    expect(sessionId).toBeTruthy();
    await apiContext.dispose();

    // ── 2. Wait for all nodes to reach "running" state ────────────────────
    // AV1 encoding (especially SVT-AV1 init) can take longer than VP9,
    // so we use a generous timeout.

    const pipeline = await pollPipeline(
      baseURL!,
      sessionId!,
      (p) => allNodesRunning(p) || !noNodesFailed(p),
      120_000
    );

    // Assert no nodes failed
    expect(
      noNodesFailed(pipeline),
      `Some nodes entered failed state: ${JSON.stringify(pipeline.nodes)}`
    ).toBe(true);

    // Assert all nodes are running
    expect(
      allNodesRunning(pipeline),
      `Not all nodes reached Running state: ${JSON.stringify(pipeline.nodes)}`
    ).toBe(true);

    // ── 3. Verify specific AV1-related nodes are present ──────────────────

    const nodeKinds = Object.entries(pipeline.nodes).map(([id, n]) => ({
      id,
      kind: n.kind,
    }));

    // AV1 decoder (rav1d) should exist
    expect(
      nodeKinds.some((n) => n.kind === 'video::av1::decoder'),
      'Expected a video::av1::decoder node'
    ).toBe(true);

    // SVT-AV1 encoder should exist
    expect(
      nodeKinds.some((n) => n.kind === 'video::svt_av1::encoder'),
      'Expected a video::svt_av1::encoder node'
    ).toBe(true);

    // Compositor should exist
    expect(
      nodeKinds.some((n) => n.kind === 'video::compositor'),
      'Expected a video::compositor node'
    ).toBe(true);

    // Publisher to relay should exist (2: source + processed output)
    const publisherNodes = nodeKinds.filter((n) => n.kind === 'transport::moq::publisher');
    expect(
      publisherNodes.length,
      `Expected 2 transport::moq::publisher nodes, found ${publisherNodes.length}`
    ).toBe(2);

    // Subscriber from relay should exist
    expect(
      nodeKinds.some((n) => n.kind === 'transport::moq::subscriber'),
      'Expected a transport::moq::subscriber node'
    ).toBe(true);

    // ── 4. Let the pipeline run for a few seconds to confirm stability ────

    await new Promise((resolve) => setTimeout(resolve, 5_000));

    // Re-check that no nodes have failed after running
    const finalPipeline = await pollPipeline(baseURL!, sessionId!, () => true, 5_000);
    expect(noNodesFailed(finalPipeline), 'Nodes failed during sustained run').toBe(true);
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
