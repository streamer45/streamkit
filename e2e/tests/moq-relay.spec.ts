// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * E2E tests for MoQ relay pub/sub pipelines.
 *
 * These tests validate that StreamKit's `transport::moq::publisher` and
 * `transport::moq::subscriber` nodes can publish to and subscribe from an
 * external moq-relay server.  They are API-level tests (no browser UI needed)
 * that create self-contained pipelines via the REST API and verify packet flow
 * through node state polling.
 *
 * Prerequisites:
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

test.describe('MoQ Relay — Audio Echo Pipeline', () => {
  let sessionId: string | null = null;

  test('creates self-contained audio relay pipeline, verifies all nodes reach running state', async ({
    baseURL,
  }) => {
    test.setTimeout(120_000);

    const relayUrl = process.env.E2E_RELAY_URL;
    if (!relayUrl) {
      test.skip(true, 'moq-relay not available (E2E_RELAY_URL not set)');
      return;
    }

    const yaml = loadPipelineFixture('moq-relay-echo.yaml', relayUrl);

    const apiContext = await request.newContext({
      baseURL: baseURL!,
      extraHTTPHeaders: getAuthHeaders(),
    });

    const sessionName = `relay-echo-test-${Date.now()}`;
    const createResponse = await apiContext.post('/api/v1/sessions', {
      data: { name: sessionName, yaml },
    });

    const responseText = await createResponse.text();
    expect(createResponse.ok(), `Failed to create session: ${responseText}`).toBeTruthy();

    const createData = JSON.parse(responseText) as { session_id: string };
    sessionId = createData.session_id;
    expect(sessionId).toBeTruthy();
    await apiContext.dispose();

    const pipeline = await pollPipeline(
      baseURL!,
      sessionId!,
      (p) => allNodesRunning(p) || !noNodesFailed(p),
      60_000
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

    const nodeKinds = Object.entries(pipeline.nodes).map(([id, n]) => ({
      id,
      kind: n.kind,
    }));

    // Publisher to relay should exist
    expect(
      nodeKinds.some((n) => n.kind === 'transport::moq::publisher'),
      'Expected at least one transport::moq::publisher node'
    ).toBe(true);

    // Subscriber from relay should exist
    expect(
      nodeKinds.some((n) => n.kind === 'transport::moq::subscriber'),
      'Expected at least one transport::moq::subscriber node'
    ).toBe(true);

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

test.describe('MoQ Relay — Multitrack (Audio + Video) Pipeline', () => {
  let sessionId: string | null = null;

  test('creates self-contained multitrack relay pipeline, verifies all nodes reach running state', async ({
    baseURL,
  }) => {
    test.setTimeout(180_000);

    const relayUrl = process.env.E2E_RELAY_URL;
    if (!relayUrl) {
      test.skip(true, 'moq-relay not available (E2E_RELAY_URL not set)');
      return;
    }

    const yaml = loadPipelineFixture('moq-relay-multitrack.yaml', relayUrl);

    const apiContext = await request.newContext({
      baseURL: baseURL!,
      extraHTTPHeaders: getAuthHeaders(),
    });

    const sessionName = `relay-multitrack-test-${Date.now()}`;
    const createResponse = await apiContext.post('/api/v1/sessions', {
      data: { name: sessionName, yaml },
    });

    const responseText = await createResponse.text();
    expect(createResponse.ok(), `Failed to create session: ${responseText}`).toBeTruthy();

    const createData = JSON.parse(responseText) as { session_id: string };
    sessionId = createData.session_id;
    expect(sessionId).toBeTruthy();
    await apiContext.dispose();
    // Multitrack pipelines need more time for video encoding to start and
    // for the subscriber to discover both audio and video tracks from the
    // relay's catalog.

    const pipeline = await pollPipeline(
      baseURL!,
      sessionId!,
      (p) => allNodesRunning(p) || !noNodesFailed(p),
      90_000
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

    const publisherNodes = Object.entries(pipeline.nodes).filter(
      ([, n]) => n.kind === 'transport::moq::publisher'
    );
    const subscriberNodes = Object.entries(pipeline.nodes).filter(
      ([, n]) => n.kind === 'transport::moq::subscriber'
    );

    // Should have 2 publishers (source → relay, and processed → relay)
    expect(
      publisherNodes.length,
      `Expected 2 transport::moq::publisher nodes, found ${publisherNodes.length}`
    ).toBe(2);

    // Should have 1 subscriber (relay → pipeline)
    expect(
      subscriberNodes.length,
      `Expected 1 transport::moq::subscriber node, found ${subscriberNodes.length}`
    ).toBe(1);

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
