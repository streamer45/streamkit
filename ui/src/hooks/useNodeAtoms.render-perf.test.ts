// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Render-performance regression test for the node-atom subscription pattern.
 *
 * Verifies the critical invariant: node components subscribe to state atoms
 * (via useNodeStateFromAtom) but NOT to params atoms.  Params are read from
 * the control level (useNumericSlider, useTuneNode) so that slider drags
 * only re-render the affected control, not the entire node subtree.
 *
 * This test catches the class of regression where adding a params atom
 * subscription to node components caused 27× full-subtree re-renders
 * during a single slider drag.
 */

import { act } from '@testing-library/react';
import { describe, it, expect, beforeEach } from 'vitest';

import {
  sessionStore,
  nodeKey,
  nodeStateAtom,
  nodeParamsAtom,
  writeNodeParam,
  batchWriteNodeStates,
} from '@/stores/sessionAtoms';
import { measureHookRenders } from '@/test/perf';
import type { NodeState } from '@/types/types';

import { useNodeStateFromAtom } from './useNodeAtoms';

// ── Setup ───────────────────────────────────────────────────────────────────

const SESSION_ID = 'test-session';
const NODE_ID = 'test-node';

function resetTestAtoms(): void {
  const key = nodeKey(SESSION_ID, NODE_ID);
  sessionStore.set(nodeStateAtom(key), null);
  sessionStore.set(nodeParamsAtom(key), {});
}

describe('useNodeStateFromAtom render-performance', () => {
  beforeEach(resetTestAtoms);

  it('does NOT re-render when params atom changes (slider isolation)', () => {
    // Seed initial state so the atom is populated.
    const seed = new Map<string, Record<string, NodeState>>();
    seed.set(SESSION_ID, { [NODE_ID]: 'Running' as NodeState });
    batchWriteNodeStates(seed);

    const result = measureHookRenders(
      (props: { nodeId: string; sessionId: string }) =>
        useNodeStateFromAtom(props.nodeId, props.sessionId, undefined),
      {
        initialProps: { nodeId: NODE_ID, sessionId: SESSION_ID },
        scenario: () => {
          // Simulate 20 rapid slider drags — each calls writeNodeParam.
          // If the hook subscribed to nodeParamsAtom, this would cause 20
          // re-renders.  Since it only subscribes to nodeStateAtom, render
          // count should stay at 1 (mount only).
          for (let i = 0; i < 20; i++) {
            act(() => {
              writeNodeParam(NODE_ID, 'opacity', 0.5 + i * 0.02, SESSION_ID);
            });
          }
        },
      }
    );
    // Mount + atom subscription sync = 2 renders.  The 20 param atom
    // writes must NOT add any renders.  With the regression (subscribing
    // to nodeParamsAtom), this would be 22+.
    expect(result.meanRenderCount).toBeLessThanOrEqual(2);
  });

  it('DOES re-render when state atom changes (state transitions work)', () => {
    const result = measureHookRenders(
      (props: { nodeId: string; sessionId: string }) =>
        useNodeStateFromAtom(props.nodeId, props.sessionId, undefined),
      {
        initialProps: { nodeId: NODE_ID, sessionId: SESSION_ID },
        scenario: () => {
          // Simulate state transitions: Initializing → Running → Ready → Running.
          const transitions: NodeState[] = ['Initializing', 'Running', 'Ready', 'Running'];
          for (const state of transitions) {
            act(() => {
              const updates = new Map<string, Record<string, NodeState>>();
              updates.set(SESSION_ID, { [NODE_ID]: state });
              batchWriteNodeStates(updates);
            });
          }
        },
      }
    );
    // Mount + subscription sync + 4 state transitions = up to 6.
    // All four transitions are distinct from the previous atom value
    // (null→Init, Init→Running, Running→Ready, Ready→Running), so
    // deepEqual does not deduplicate any of them.
    // Allow headroom for React batching variance.
    expect(result.meanRenderCount).toBeGreaterThanOrEqual(3);
    expect(result.meanRenderCount).toBeLessThanOrEqual(7);
  });

  it('rapid param writes with interleaved state change: only state triggers re-render', () => {
    const seed2 = new Map<string, Record<string, NodeState>>();
    seed2.set(SESSION_ID, { [NODE_ID]: 'Running' as NodeState });
    batchWriteNodeStates(seed2);

    const result = measureHookRenders(
      (props: { nodeId: string; sessionId: string }) =>
        useNodeStateFromAtom(props.nodeId, props.sessionId, undefined),
      {
        initialProps: { nodeId: NODE_ID, sessionId: SESSION_ID },
        scenario: () => {
          // 10 param writes (should NOT trigger re-renders)
          for (let i = 0; i < 10; i++) {
            act(() => {
              writeNodeParam(NODE_ID, 'gain_db', -6 + i, SESSION_ID);
            });
          }
          // 1 state change (SHOULD trigger re-render)
          act(() => {
            const updates = new Map<string, Record<string, NodeState>>();
            updates.set(SESSION_ID, { [NODE_ID]: 'Ready' as NodeState });
            batchWriteNodeStates(updates);
          });
          // 10 more param writes (should NOT trigger re-renders)
          for (let i = 0; i < 10; i++) {
            act(() => {
              writeNodeParam(NODE_ID, 'gain_db', i, SESSION_ID);
            });
          }
        },
      }
    );
    // Mount + subscription sync + 1 state change = 3.  The 20 param
    // writes must not contribute to the render count.  With the
    // regression, this would be 23+.
    expect(result.meanRenderCount).toBeLessThanOrEqual(4);
  });
});
