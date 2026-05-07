// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, it, expect, beforeEach, vi } from 'vitest';

import { sessionStore, nodeStateAtom, nodeKey } from '@/stores/sessionAtoms';
import { useSessionStore } from '@/stores/sessionStore';
import type { Pipeline, NodeState } from '@/types/types';

function makePipeline(nodes: Record<string, { kind: string; state?: NodeState | null }>): Pipeline {
  const mapped: Pipeline['nodes'] = {};
  for (const [id, n] of Object.entries(nodes)) {
    mapped[id] = { kind: n.kind, params: {}, state: n.state ?? null };
  }
  return {
    name: null,
    description: null,
    mode: 'dynamic',
    client: null,
    nodes: mapped,
    connections: [{ from_node: 'source', from_pin: 'out', to_node: 'mixer', to_pin: 'audio_in' }],
  };
}

describe('useEdgeAlertSubscription — Jotai atom integration', () => {
  const SESSION_ID = 'test-session-edge-alerts';

  beforeEach(() => {
    useSessionStore.setState({ sessions: new Map() });
    // Clear atoms from previous tests
    for (const key of [...nodeStateAtom.getParams()]) {
      nodeStateAtom.remove(key);
    }
  });

  it('should write node states to Jotai atoms via batchWriteNodeStates', async () => {
    const { batchWriteNodeStates } = await import('@/stores/sessionAtoms');

    const updates = new Map<string, Record<string, NodeState>>();
    updates.set(SESSION_ID, {
      source: 'Running',
      mixer: 'Running',
    });
    batchWriteNodeStates(updates);

    expect(sessionStore.get(nodeStateAtom(nodeKey(SESSION_ID, 'source')))).toBe('Running');
    expect(sessionStore.get(nodeStateAtom(nodeKey(SESSION_ID, 'mixer')))).toBe('Running');
  });

  it('should notify Jotai subscribers when node state changes', async () => {
    const { batchWriteNodeStates } = await import('@/stores/sessionAtoms');

    const key = nodeKey(SESSION_ID, 'mixer');
    const atom = nodeStateAtom(key);
    const callback = vi.fn();

    sessionStore.sub(atom, callback);

    const updates = new Map<string, Record<string, NodeState>>();
    updates.set(SESSION_ID, { mixer: 'Running' });
    batchWriteNodeStates(updates);

    expect(callback).toHaveBeenCalled();
  });

  it('should not notify Jotai subscribers when value is deeply equal', async () => {
    const { batchWriteNodeStates } = await import('@/stores/sessionAtoms');

    const key = nodeKey(SESSION_ID, 'mixer');
    const atom = nodeStateAtom(key);

    // Set initial value
    const initial = new Map<string, Record<string, NodeState>>();
    initial.set(SESSION_ID, { mixer: 'Running' });
    batchWriteNodeStates(initial);

    const callback = vi.fn();
    sessionStore.sub(atom, callback);

    // Write same value again — batchWriteNodeStates has a deepEqual guard
    const same = new Map<string, Record<string, NodeState>>();
    same.set(SESSION_ID, { mixer: 'Running' });
    batchWriteNodeStates(same);

    expect(callback).not.toHaveBeenCalled();
  });

  it('should detect degraded slow_input_timeout state in Jotai atoms', async () => {
    const { batchWriteNodeStates } = await import('@/stores/sessionAtoms');
    const { extractSlowTimeoutDetailsFromNodeState } = await import('@/utils/pipelineGraph');

    const degradedState: NodeState = {
      Degraded: {
        reason: 'slow_input_timeout',
        details: {
          slow_pins: ['audio_in'],
          newly_slow_pins: ['audio_in'],
          sync_timeout_ms: 100,
        },
      },
    };

    const updates = new Map<string, Record<string, NodeState>>();
    updates.set(SESSION_ID, { mixer: degradedState });
    batchWriteNodeStates(updates);

    const stored = sessionStore.get(nodeStateAtom(nodeKey(SESSION_ID, 'mixer')));
    const details = extractSlowTimeoutDetailsFromNodeState(stored);

    expect(details).not.toBeNull();
    expect(details?.slowPins).toEqual(['audio_in']);
    expect(details?.newlySlowPins).toEqual(['audio_in']);
    expect(details?.syncTimeoutMs).toBe(100);
  });

  it('should clear edge alert data when node recovers from degraded state', async () => {
    const { batchWriteNodeStates } = await import('@/stores/sessionAtoms');
    const { extractSlowTimeoutDetailsFromNodeState } = await import('@/utils/pipelineGraph');

    const degradedState: NodeState = {
      Degraded: {
        reason: 'slow_input_timeout',
        details: { slow_pins: ['audio_in'], newly_slow_pins: [], sync_timeout_ms: 100 },
      },
    };

    const updates1 = new Map<string, Record<string, NodeState>>();
    updates1.set(SESSION_ID, { mixer: degradedState });
    batchWriteNodeStates(updates1);

    // Recover
    const updates2 = new Map<string, Record<string, NodeState>>();
    updates2.set(SESSION_ID, { mixer: 'Running' });
    batchWriteNodeStates(updates2);

    const stored = sessionStore.get(nodeStateAtom(nodeKey(SESSION_ID, 'mixer')));
    const details = extractSlowTimeoutDetailsFromNodeState(stored);
    expect(details).toBeNull();
  });

  it('should read node states from atoms for all pipeline nodes', () => {
    const pipeline = makePipeline({
      source: { kind: 'core::passthrough', state: 'Running' },
      mixer: { kind: 'core::mixer', state: 'Running' },
    });

    // Seed atoms
    sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'source')), 'Running');
    sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'mixer')), 'Running');

    const states = new Map<string, NodeState | null>();
    for (const id of Object.keys(pipeline.nodes)) {
      states.set(id, sessionStore.get(nodeStateAtom(nodeKey(SESSION_ID, id))));
    }

    expect(states.get('source')).toBe('Running');
    expect(states.get('mixer')).toBe('Running');
  });
});
