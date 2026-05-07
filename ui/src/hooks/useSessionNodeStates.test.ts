// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, it, expect, beforeEach } from 'vitest';

import { sessionStore, nodeStateAtom, nodeKey } from '@/stores/sessionAtoms';
import { useSessionStore } from '@/stores/sessionStore';
import type { Pipeline, NodeState } from '@/types/types';

describe('useSessionNodeStates — Jotai atom aggregation', () => {
  const SESSION_ID = 'test-session-node-states';

  beforeEach(() => {
    useSessionStore.setState({ sessions: new Map() });
    for (const key of [...nodeStateAtom.getParams()]) {
      nodeStateAtom.remove(key);
    }
  });

  function seedPipeline(nodes: Record<string, { kind: string; state?: NodeState | null }>) {
    const mapped: Pipeline['nodes'] = {};
    for (const [id, n] of Object.entries(nodes)) {
      mapped[id] = { kind: n.kind, params: {}, state: n.state ?? null };
    }
    const pipeline: Pipeline = {
      name: null,
      description: null,
      mode: 'dynamic',
      client: null,
      nodes: mapped,
      connections: [],
    };
    useSessionStore.getState().setPipeline(SESSION_ID, pipeline);
    return pipeline;
  }

  it('should read node states from Jotai atoms matching pipeline node IDs', () => {
    seedPipeline({
      source: { kind: 'core::passthrough' },
      mixer: { kind: 'core::mixer' },
    });

    sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'source')), 'Running');
    sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'mixer')), 'Initializing');

    const nodeIds = Object.keys(
      useSessionStore.getState().getSession(SESSION_ID)?.pipeline?.nodes ?? {}
    );

    const states: Record<string, NodeState> = {};
    for (const id of nodeIds) {
      const s = sessionStore.get(nodeStateAtom(nodeKey(SESSION_ID, id)));
      if (s != null) states[id] = s;
    }

    expect(states).toEqual({ source: 'Running', mixer: 'Initializing' });
  });

  it('should exclude nodes without state (null atoms)', () => {
    seedPipeline({
      source: { kind: 'core::passthrough' },
      mixer: { kind: 'core::mixer' },
    });

    // Only set state for source, leave mixer as null
    sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'source')), 'Running');

    const nodeIds = Object.keys(
      useSessionStore.getState().getSession(SESSION_ID)?.pipeline?.nodes ?? {}
    );

    const states: Record<string, NodeState> = {};
    for (const id of nodeIds) {
      const s = sessionStore.get(nodeStateAtom(nodeKey(SESSION_ID, id)));
      if (s != null) states[id] = s;
    }

    expect(states).toEqual({ source: 'Running' });
    expect(states['mixer']).toBeUndefined();
  });

  it('should support computing session status from aggregated states', async () => {
    const { computeSessionStatus } = await import('@/utils/sessionStatus');

    seedPipeline({
      source: { kind: 'core::passthrough' },
      mixer: { kind: 'core::mixer' },
    });

    sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'source')), 'Running');
    sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'mixer')), 'Running');

    const nodeIds = Object.keys(
      useSessionStore.getState().getSession(SESSION_ID)?.pipeline?.nodes ?? {}
    );

    const states: Record<string, NodeState> = {};
    for (const id of nodeIds) {
      const s = sessionStore.get(nodeStateAtom(nodeKey(SESSION_ID, id)));
      if (s != null) states[id] = s;
    }

    expect(computeSessionStatus(states)).toBe('running');
  });

  it('should detect degraded session from Jotai atom states', async () => {
    const { computeSessionStatus } = await import('@/utils/sessionStatus');

    seedPipeline({
      source: { kind: 'core::passthrough' },
      mixer: { kind: 'core::mixer' },
    });

    sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'source')), 'Running');
    sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'mixer')), {
      Degraded: { reason: 'slow_input_timeout', details: null },
    });

    const nodeIds = Object.keys(
      useSessionStore.getState().getSession(SESSION_ID)?.pipeline?.nodes ?? {}
    );

    const states: Record<string, NodeState> = {};
    for (const id of nodeIds) {
      const s = sessionStore.get(nodeStateAtom(nodeKey(SESSION_ID, id)));
      if (s != null) states[id] = s;
    }

    expect(computeSessionStatus(states)).toBe('degraded');
  });
});
