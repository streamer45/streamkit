// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { act, render, renderHook, waitFor } from '@testing-library/react';
import { ReactFlowProvider, useStoreApi, type Node } from '@xyflow/react';
import React, { useEffect } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { PinHandle } from '@/components/node/PinHandle';
import { PinRow } from '@/components/node/PinRow';
import { sessionStore, nodeKey, nodeStateAtom } from '@/stores/sessionAtoms';
import type { Connection, NodeState, OutputPin } from '@/types/types';

import { buildSlowInputAlert, useSlowInputAlert } from './TypedEdge';

vi.mock('@/components/node/PinHandle', () => ({
  PinHandle: vi.fn(({ name, packetType }: { name: string; packetType: unknown }) => (
    <span data-testid={`pin-${name}`}>{JSON.stringify(packetType)}</span>
  )),
}));

const SESSION_ID = 'monitor-perf-session';
const TARGET_NODE = 'mixer';
const edgeContext = {
  sessionId: SESSION_ID,
  connections: [
    { from_node: 'source', from_pin: 'audio', to_node: TARGET_NODE, to_pin: 'audio_in' },
    { from_node: 'source', from_pin: 'video', to_node: TARGET_NODE, to_pin: 'video_in' },
  ] satisfies Connection[],
};

const edge = {
  source: 'source',
  sourceHandle: 'audio',
  target: TARGET_NODE,
  targetHandle: 'audio_in',
};

function slowState(
  slowPins: string[],
  newlySlowPins: string[] = slowPins,
  syncTimeoutMs = 500
): NodeState {
  return {
    Degraded: {
      reason: 'slow_input_timeout',
      details: {
        slow_pins: slowPins,
        newly_slow_pins: newlySlowPins,
        sync_timeout_ms: syncTimeoutMs,
      },
    },
  };
}

function resetTargetState(): void {
  sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, TARGET_NODE)), null);
}

function AlertProbe({
  targetHandle,
  onRender,
}: {
  targetHandle: string;
  onRender: (alert: ReturnType<typeof useSlowInputAlert>) => void;
}) {
  const alert = useSlowInputAlert({ ...edge, targetHandle }, edgeContext);
  onRender(alert);
  return null;
}

describe('Monitor edge alert render isolation', () => {
  beforeEach(resetTargetState);

  it('updates only the exact target pin and ignores unrelated nodes', () => {
    const audioRenders: ReturnType<typeof useSlowInputAlert>[] = [];
    const videoRenders: ReturnType<typeof useSlowInputAlert>[] = [];
    render(
      <>
        <AlertProbe targetHandle="audio_in" onRender={(alert) => audioRenders.push(alert)} />
        <AlertProbe targetHandle="video_in" onRender={(alert) => videoRenders.push(alert)} />
      </>
    );
    const initialVideoRenders = videoRenders.length;

    act(() => {
      sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, TARGET_NODE)), slowState(['audio_in']));
    });

    expect(audioRenders.at(-1)).not.toBeNull();
    expect(videoRenders.length).toBe(initialVideoRenders);
    const audioRendersAfterMatchingUpdate = audioRenders.length;
    const videoRendersAfterMatchingUpdate = videoRenders.length;

    act(() => {
      sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, 'unrelated')), slowState(['audio_in']));
    });

    expect(audioRenders.length).toBe(audioRendersAfterMatchingUpdate);
    expect(videoRenders.length).toBe(videoRendersAfterMatchingUpdate);
  });

  it('warns on matching pins, ignores nonmatching pins, and clears on recovery', () => {
    const { result } = renderHook(() => useSlowInputAlert(edge, edgeContext));

    expect(result.current).toBeNull();
    act(() => {
      sessionStore.set(
        nodeStateAtom(nodeKey(SESSION_ID, TARGET_NODE)),
        slowState(['video_in'], ['video_in'])
      );
    });
    expect(result.current).toBeNull();

    act(() => {
      sessionStore.set(
        nodeStateAtom(nodeKey(SESSION_ID, TARGET_NODE)),
        slowState(['audio_in'], ['audio_in'])
      );
    });
    expect(result.current?.kind).toBe('slow_input_timeout');
    expect(result.current?.severity).toBe('warning');

    act(() => {
      sessionStore.set(nodeStateAtom(nodeKey(SESSION_ID, TARGET_NODE)), 'Running');
    });
    expect(result.current).toBeNull();
  });

  it('keeps the existing tooltip title and source detail lines', () => {
    const alert = buildSlowInputAlert(
      edge,
      {
        slowPins: ['audio_in', 'missing_in'],
        newlySlowPins: ['audio_in'],
        syncTimeoutMs: 750,
      },
      edgeContext.connections
    );

    expect(alert).toEqual({
      kind: 'slow_input_timeout',
      severity: 'warning',
      tooltip: {
        title: 'mixer degraded',
        lines: [
          'Slow inputs: source.audio → audio_in',
          'This: source.audio → audio_in',
          'Newly slow: audio_in',
          'Timeout: 750ms',
        ],
      },
    });
  });
});

let flowStore: ReturnType<typeof useStoreApi> | null = null;

function StoreProbe() {
  const store = useStoreApi();
  useEffect(() => {
    flowStore = store;
  }, [store]);
  return null;
}

function FlowHarness({ children }: { children: React.ReactNode }) {
  return (
    <ReactFlowProvider>
      <StoreProbe />
      {children}
    </ReactFlowProvider>
  );
}

const passthroughPin: OutputPin = {
  name: 'out',
  produces_type: 'Passthrough',
  cardinality: 'One',
};

const sourceNode: Node = {
  id: 'source',
  type: 'test',
  position: { x: 0, y: 0 },
  data: { inputs: [], outputs: [{ name: 'out', produces_type: 'Text', cardinality: 'One' }] },
};

const passthroughNode: Node = {
  id: 'passthrough',
  type: 'test',
  position: { x: 0, y: 0 },
  data: { inputs: [{ name: 'in', accepts_types: ['Any'], cardinality: 'One' }], outputs: [] },
};

describe('PinRow render isolation', () => {
  beforeEach(() => {
    flowStore = null;
  });

  it('ignores edge-data-only updates while following relevant topology type changes', async () => {
    const pinHandleRenders = vi.fn();
    vi.mocked(PinHandle).mockImplementation(({ name, packetType }) => {
      pinHandleRenders({ name, packetType });
      return <span data-testid={`pin-${name}`}>{JSON.stringify(packetType)}</span>;
    });

    render(
      <FlowHarness>
        <PinRow nodeId="passthrough" side="right" pins={[passthroughPin]} isInput={false} />
      </FlowHarness>
    );
    await waitFor(() => expect(flowStore).not.toBeNull());

    act(() => {
      flowStore!.getState().setNodes([sourceNode, passthroughNode]);
      flowStore!.getState().setEdges([
        {
          id: 'source-out-passthrough-in',
          source: 'source',
          sourceHandle: 'out',
          target: 'passthrough',
          targetHandle: 'in',
          data: { alert: { kind: 'slow_input_timeout' } },
        },
      ]);
    });
    await waitFor(() => expect(pinHandleRenders).toHaveBeenCalled());
    pinHandleRenders.mockClear();

    act(() => {
      flowStore!.getState().setEdges([
        {
          id: 'source-out-passthrough-in',
          source: 'source',
          sourceHandle: 'out',
          target: 'passthrough',
          targetHandle: 'in',
          data: { alert: { kind: 'other' } },
        },
      ]);
    });
    expect(pinHandleRenders).not.toHaveBeenCalled();

    act(() => {
      flowStore!.getState().setNodes([
        {
          ...sourceNode,
          data: {
            ...sourceNode.data,
            outputs: [{ name: 'out', produces_type: 'Binary', cardinality: 'One' }],
          },
        },
        passthroughNode,
      ]);
    });
    await waitFor(() =>
      expect(pinHandleRenders).toHaveBeenCalledWith(
        expect.objectContaining({ packetType: 'Binary' })
      )
    );
  });

  it('returns null for rows without passthrough outputs', async () => {
    const pinHandleRenders = vi.fn();
    vi.mocked(PinHandle).mockImplementation(({ name, packetType }) => {
      pinHandleRenders({ name, packetType });
      return <span data-testid={`pin-${name}`}>{JSON.stringify(packetType)}</span>;
    });

    render(
      <FlowHarness>
        <PinRow
          nodeId="regular"
          side="right"
          pins={[{ name: 'out', produces_type: 'Text', cardinality: 'One' }]}
          isInput={false}
        />
      </FlowHarness>
    );
    await waitFor(() =>
      expect(pinHandleRenders).toHaveBeenCalledWith(expect.objectContaining({ packetType: 'Text' }))
    );
  });
});
