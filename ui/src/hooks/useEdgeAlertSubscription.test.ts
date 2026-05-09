// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { renderHook, act } from '@testing-library/react';
import type { Edge } from '@xyflow/react';
import React from 'react';
import { describe, it, expect, afterEach, vi } from 'vitest';

import { sessionStore, nodeStateAtom, nodeKey, clearSessionAtoms } from '@/stores/sessionAtoms';
import { useSessionStore } from '@/stores/sessionStore';
import type { Pipeline, NodeState } from '@/types/types';
import { isRecord } from '@/utils/pipelineGraph';

import type { UseEdgeAlertSubscriptionOptions } from './useEdgeAlertSubscription';
import { useEdgeAlertSubscription } from './useEdgeAlertSubscription';

const SESSION_ID = 'test-session-edge-alerts';

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

function makeEdges(): Edge[] {
  return [
    {
      id: 'source-mixer',
      source: 'source',
      sourceHandle: 'out',
      target: 'mixer',
      targetHandle: 'audio_in',
    },
  ];
}

function makeOptions(
  overrides: Partial<UseEdgeAlertSubscriptionOptions> & {
    pipeline?: Pipeline;
    edges?: Edge[];
  } = {}
): {
  options: UseEdgeAlertSubscriptionOptions;
  getEdges: () => Edge[];
} {
  const pipeline =
    overrides.pipeline ??
    makePipeline({
      source: { kind: 'core::passthrough' },
      mixer: { kind: 'core::mixer' },
    });
  let edges = overrides.edges ?? makeEdges();
  const setEdges: React.Dispatch<React.SetStateAction<Edge[]>> = (updater) => {
    edges = typeof updater === 'function' ? updater(edges) : updater;
  };
  const pipelineRef = { current: pipeline };

  return {
    options: {
      selectedSessionId: overrides.selectedSessionId ?? SESSION_ID,
      setEdges: overrides.setEdges ?? setEdges,
      pipelineRef: pipelineRef as React.RefObject<Pipeline | undefined | null>,
      topoKey: overrides.topoKey ?? 'topo-1',
    },
    getEdges: () => edges,
  };
}

afterEach(() => {
  clearSessionAtoms(SESSION_ID);
  useSessionStore.getState().clearSession(SESSION_ID);
});

describe('useEdgeAlertSubscription', () => {
  it('returns topoEffectRanRef that gates edge patching', () => {
    const { options, getEdges } = makeOptions();

    const { result } = renderHook(() => useEdgeAlertSubscription(options));

    // Initially false — patches should be gated
    expect(result.current.topoEffectRanRef.current).toBe(false);

    // Write a degraded state — should NOT patch because gate is closed
    act(() => {
      sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'mixer')), {
        Degraded: {
          reason: 'slow_input_timeout',
          details: { slow_pins: ['audio_in'], newly_slow_pins: ['audio_in'], sync_timeout_ms: 100 },
        },
      });
    });

    const edgesAfterGated = getEdges();
    const alert = isRecord(edgesAfterGated[0].data) ? edgesAfterGated[0].data['alert'] : undefined;
    expect(alert).toBeUndefined();
  });

  it('patches edges with alert data when a node enters slow_input_timeout', () => {
    const { options, getEdges } = makeOptions();

    const { result } = renderHook(() => useEdgeAlertSubscription(options));

    // Open the gate (simulates topology effect having run)
    act(() => {
      result.current.topoEffectRanRef.current = true;
    });

    act(() => {
      sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'mixer')), {
        Degraded: {
          reason: 'slow_input_timeout',
          details: { slow_pins: ['audio_in'], newly_slow_pins: ['audio_in'], sync_timeout_ms: 100 },
        },
      });
    });

    const patched = getEdges();
    const alertData = isRecord(patched[0].data) ? patched[0].data['alert'] : undefined;
    expect(alertData).toBeDefined();
    expect(isRecord(alertData) && alertData['kind']).toBe('slow_input_timeout');
  });

  it('clears edge alert when node recovers from degraded state', () => {
    const { options, getEdges } = makeOptions();

    const { result } = renderHook(() => useEdgeAlertSubscription(options));

    act(() => {
      result.current.topoEffectRanRef.current = true;
    });

    // Enter degraded state
    act(() => {
      sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'mixer')), {
        Degraded: {
          reason: 'slow_input_timeout',
          details: { slow_pins: ['audio_in'], newly_slow_pins: [], sync_timeout_ms: 100 },
        },
      });
    });

    const degraded = getEdges();
    const alertBefore = isRecord(degraded[0].data) ? degraded[0].data['alert'] : undefined;
    expect(alertBefore).toBeDefined();

    // Recover
    act(() => {
      sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'mixer')), 'Running');
    });

    const recovered = getEdges();
    const alertAfter = isRecord(recovered[0].data) ? recovered[0].data['alert'] : undefined;
    expect(alertAfter).toBeUndefined();
  });

  it('does not patch edges for a non-matching target handle', () => {
    const edges: Edge[] = [
      {
        id: 'source-mixer',
        source: 'source',
        sourceHandle: 'out',
        target: 'mixer',
        targetHandle: 'video_in', // not the slow pin
      },
    ];
    const { options, getEdges } = makeOptions({ edges });

    const { result } = renderHook(() => useEdgeAlertSubscription(options));

    act(() => {
      result.current.topoEffectRanRef.current = true;
    });

    act(() => {
      sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'mixer')), {
        Degraded: {
          reason: 'slow_input_timeout',
          details: { slow_pins: ['audio_in'], newly_slow_pins: ['audio_in'], sync_timeout_ms: 100 },
        },
      });
    });

    const patched = getEdges();
    const alert = isRecord(patched[0].data) ? patched[0].data['alert'] : undefined;
    expect(alert).toBeUndefined();
  });

  it('resets topoEffectRanRef when selectedSessionId changes', () => {
    const { options } = makeOptions();

    const { result, rerender } = renderHook(
      (props: UseEdgeAlertSubscriptionOptions) => useEdgeAlertSubscription(props),
      { initialProps: options }
    );

    act(() => {
      result.current.topoEffectRanRef.current = true;
    });
    expect(result.current.topoEffectRanRef.current).toBe(true);

    // Change session — effect re-runs and resets the gate
    const newOptions = { ...options, selectedSessionId: 'other-session' };
    rerender(newOptions);

    expect(result.current.topoEffectRanRef.current).toBe(false);
  });

  it('uses React.startTransition for edge updates', () => {
    const startTransitionSpy = vi.spyOn(React, 'startTransition');
    const { options } = makeOptions();

    const { result } = renderHook(() => useEdgeAlertSubscription(options));

    act(() => {
      result.current.topoEffectRanRef.current = true;
    });

    act(() => {
      sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'mixer')), {
        Degraded: {
          reason: 'slow_input_timeout',
          details: { slow_pins: ['audio_in'], newly_slow_pins: [], sync_timeout_ms: 100 },
        },
      });
    });

    expect(startTransitionSpy).toHaveBeenCalled();
    startTransitionSpy.mockRestore();
  });
});
