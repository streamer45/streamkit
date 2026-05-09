// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { renderHook, act } from '@testing-library/react';
import { describe, it, expect, afterEach } from 'vitest';

import { sessionStore, nodeStateAtom, nodeKey, clearSessionAtoms } from '@/stores/sessionAtoms';
import { useSessionStore } from '@/stores/sessionStore';
import type { Pipeline, NodeState } from '@/types/types';

import { useSessionNodeStates } from './useSessionNodeStates';

const SESSION_ID = 'test-session-node-states';

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
  useSessionStore.getState().initSession(SESSION_ID, true);
  useSessionStore.getState().setPipeline(SESSION_ID, pipeline);
}

afterEach(() => {
  clearSessionAtoms(SESSION_ID);
  useSessionStore.getState().clearSession(SESSION_ID);
});

describe('useSessionNodeStates', () => {
  it('aggregates per-node Jotai atoms into a session-level record', () => {
    seedPipeline({
      source: { kind: 'core::passthrough' },
      mixer: { kind: 'core::mixer' },
    });

    sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'source')), 'Running');
    sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'mixer')), 'Initializing');

    const { result } = renderHook(() => useSessionNodeStates(SESSION_ID));

    expect(result.current).toEqual({ source: 'Running', mixer: 'Initializing' });
  });

  it('excludes nodes whose atom is null', () => {
    seedPipeline({
      source: { kind: 'core::passthrough' },
      mixer: { kind: 'core::mixer' },
    });

    sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'source')), 'Running');

    const { result } = renderHook(() => useSessionNodeStates(SESSION_ID));

    expect(result.current).toEqual({ source: 'Running' });
    expect(result.current['mixer']).toBeUndefined();
  });

  it('updates when a node state atom changes', () => {
    seedPipeline({
      source: { kind: 'core::passthrough' },
      mixer: { kind: 'core::mixer' },
    });

    sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'source')), 'Running');
    sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'mixer')), 'Running');

    const { result } = renderHook(() => useSessionNodeStates(SESSION_ID));
    expect(result.current).toEqual({ source: 'Running', mixer: 'Running' });

    act(() => {
      sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'mixer')), 'Initializing');
    });

    expect(result.current).toEqual({ source: 'Running', mixer: 'Initializing' });
  });

  it('returns a stable reference when atom values have not changed', () => {
    seedPipeline({
      source: { kind: 'core::passthrough' },
      mixer: { kind: 'core::mixer' },
    });

    sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'source')), 'Running');
    sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'mixer')), 'Running');

    const { result } = renderHook(() => useSessionNodeStates(SESSION_ID));
    const first = result.current;

    // Re-write the same value — batchWriteNodeStates' deepEqual guard would
    // skip the write, but even without that guard the shallow-equality guard
    // inside the aggregate atom should return the same reference.
    act(() => {
      sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'source')), 'Running');
    });

    expect(result.current).toBe(first);
  });

  it('works with computeSessionStatus', async () => {
    const { computeSessionStatus } = await import('@/utils/sessionStatus');

    seedPipeline({
      source: { kind: 'core::passthrough' },
      mixer: { kind: 'core::mixer' },
    });

    sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'source')), 'Running');
    sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'mixer')), 'Running');

    const { result } = renderHook(() => useSessionNodeStates(SESSION_ID));
    expect(computeSessionStatus(result.current)).toBe('running');
  });

  it('detects degraded session status', async () => {
    const { computeSessionStatus } = await import('@/utils/sessionStatus');

    seedPipeline({
      source: { kind: 'core::passthrough' },
      mixer: { kind: 'core::mixer' },
    });

    sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'source')), 'Running');
    sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'mixer')), {
      Degraded: { reason: 'slow_input_timeout', details: null },
    });

    const { result } = renderHook(() => useSessionNodeStates(SESSION_ID));
    expect(computeSessionStatus(result.current)).toBe('degraded');
  });
});
