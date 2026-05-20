// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Tests for useAutoLayout — exercises DAG layout, fitView scheduling, and
// the position-store persistence path against a stub ReactFlowInstance.

import { act, renderHook } from '@testing-library/react';
import type { Node as RFNode, ReactFlowInstance } from '@xyflow/react';
import React from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { Pipeline } from '@/types/types';

import { useAutoLayout } from './useAutoLayout';

function makePipeline(): Pipeline {
  return {
    name: null,
    description: null,
    mode: 'oneshot',
    nodes: {
      src: { kind: 'audio::null_source', params: {}, state: null },
      sink: { kind: 'audio::null_sink', params: {}, state: null },
    },
    connections: [{ from_node: 'src', from_pin: 'out', to_node: 'sink', to_pin: 'in' }],
  };
}

function makeNode(id: string, height = 100): RFNode {
  return {
    id,
    type: 'editor',
    position: { x: 0, y: 0 },
    data: { label: id },
    height,
    measured: { width: 250, height },
  };
}

interface StubInstance {
  fitView: ReturnType<typeof vi.fn>;
  getNodes: ReturnType<typeof vi.fn>;
}

function makeStubInstance(nodes: RFNode[] = []): {
  ref: React.RefObject<ReactFlowInstance | null>;
  instance: StubInstance;
} {
  const instance: StubInstance = {
    fitView: vi.fn(),
    getNodes: vi.fn(() => nodes),
  };
  const ref: React.RefObject<ReactFlowInstance | null> = {
    current: instance as unknown as ReactFlowInstance,
  };
  return { ref, instance };
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('useAutoLayout — applyAutoLayout', () => {
  it('computes positions, updates nodes, and persists to the position store', () => {
    const setNodes = vi.fn();
    const updateNodePosition = vi.fn();
    const { ref } = makeStubInstance();
    const pipeline = makePipeline();
    const nodes = [makeNode('src'), makeNode('sink')];

    setNodes.mockImplementation((updater: (prev: RFNode[]) => RFNode[]) => updater(nodes));

    const { result } = renderHook(() =>
      useAutoLayout({
        pipeline,
        selectedSessionId: 'sess-1',
        nodesLength: 2,
        setNodes,
        rf: ref,
        updateNodePosition,
      })
    );

    act(() => {
      result.current.applyAutoLayout({ src: 100, sink: 100 });
    });

    expect(setNodes).toHaveBeenCalledTimes(1);
    expect(updateNodePosition).toHaveBeenCalledTimes(2);
    expect(updateNodePosition).toHaveBeenCalledWith(
      'sess-1',
      'src',
      expect.objectContaining({
        x: expect.any(Number),
        y: expect.any(Number),
      })
    );
    expect(updateNodePosition).toHaveBeenCalledWith(
      'sess-1',
      'sink',
      expect.objectContaining({
        x: expect.any(Number),
        y: expect.any(Number),
      })
    );
  });

  it('produces top-down layout: source above sink', () => {
    const setNodes = vi.fn();
    const updateNodePosition = vi.fn();
    const { ref } = makeStubInstance();
    const pipeline = makePipeline();

    setNodes.mockImplementation((updater: (prev: RFNode[]) => RFNode[]) =>
      updater([makeNode('src'), makeNode('sink')])
    );

    const { result } = renderHook(() =>
      useAutoLayout({
        pipeline,
        selectedSessionId: 'sess',
        nodesLength: 2,
        setNodes,
        rf: ref,
        updateNodePosition,
      })
    );

    act(() => {
      result.current.applyAutoLayout({ src: 100, sink: 100 });
    });

    const positions = new Map<string, { x: number; y: number }>();
    for (const call of updateNodePosition.mock.calls) {
      positions.set(call[1] as string, call[2] as { x: number; y: number });
    }
    expect(positions.get('sink')!.y).toBeGreaterThan(positions.get('src')!.y);
  });

  it('returns the same node reference when its computed position matches the current one', () => {
    // First call captures the positions verticalLayout produces, second call
    // re-seeds `prev` with those positions so the map-updater hits the
    // `if (n.position.x === newPos.x && n.position.y === newPos.y) return n`
    // fast path and we can assert reference-equality.
    const setNodes = vi.fn();
    const { ref } = makeStubInstance();
    const pipeline = makePipeline();

    let computedPositions: Map<string, { x: number; y: number }> | null = null;
    let secondCallSeed: RFNode[] | null = null;
    let secondCallNext: RFNode[] | null = null;

    setNodes.mockImplementation((updater: (prev: RFNode[]) => RFNode[]) => {
      if (computedPositions === null) {
        const next = updater([makeNode('src'), makeNode('sink')]);
        computedPositions = new Map(next.map((n) => [n.id, n.position]));
        return next;
      }
      secondCallSeed = [
        { ...makeNode('src'), position: computedPositions.get('src')! },
        { ...makeNode('sink'), position: computedPositions.get('sink')! },
      ];
      secondCallNext = updater(secondCallSeed);
      return secondCallNext;
    });

    const { result } = renderHook(() =>
      useAutoLayout({
        pipeline,
        selectedSessionId: 'sess',
        nodesLength: 2,
        setNodes,
        rf: ref,
        updateNodePosition: vi.fn(),
      })
    );

    act(() => {
      result.current.applyAutoLayout({ src: 100, sink: 100 });
      result.current.applyAutoLayout({ src: 100, sink: 100 });
    });

    expect(secondCallSeed).not.toBeNull();
    expect(secondCallNext).not.toBeNull();
    expect(secondCallNext![0]).toBe(secondCallSeed![0]);
    expect(secondCallNext![1]).toBe(secondCallSeed![1]);
  });

  it('produces fresh node objects when the computed position differs from current', () => {
    const setNodes = vi.fn();
    const { ref } = makeStubInstance();
    const pipeline = makePipeline();

    let capturedSeed: RFNode[] | null = null;
    let capturedNext: RFNode[] | null = null;
    setNodes.mockImplementation((updater: (prev: RFNode[]) => RFNode[]) => {
      capturedSeed = [
        { ...makeNode('src'), position: { x: 1, y: 1 } },
        { ...makeNode('sink'), position: { x: 999, y: 999 } },
      ];
      capturedNext = updater(capturedSeed);
      return capturedNext;
    });

    const { result } = renderHook(() =>
      useAutoLayout({
        pipeline,
        selectedSessionId: 'sess',
        nodesLength: 2,
        setNodes,
        rf: ref,
        updateNodePosition: vi.fn(),
      })
    );

    act(() => {
      result.current.applyAutoLayout({ src: 100, sink: 100 });
    });

    expect(capturedNext).not.toBeNull();
    expect(capturedNext![0]).not.toBe(capturedSeed![0]);
    expect(capturedNext![1]).not.toBe(capturedSeed![1]);
  });

  it('skips persistence when selectedSessionId is null but still updates nodes', () => {
    const setNodes = vi.fn();
    const updateNodePosition = vi.fn();
    const { ref } = makeStubInstance();

    setNodes.mockImplementation((updater: (prev: RFNode[]) => RFNode[]) =>
      updater([makeNode('src'), makeNode('sink')])
    );

    const { result } = renderHook(() =>
      useAutoLayout({
        pipeline: makePipeline(),
        selectedSessionId: null,
        nodesLength: 2,
        setNodes,
        rf: ref,
        updateNodePosition,
      })
    );

    act(() => {
      result.current.applyAutoLayout({ src: 100, sink: 100 });
    });

    expect(setNodes).toHaveBeenCalledTimes(1);
    expect(updateNodePosition).not.toHaveBeenCalled();
  });

  it('is a no-op when pipeline is null', () => {
    const setNodes = vi.fn();
    const updateNodePosition = vi.fn();
    const { ref } = makeStubInstance();

    const { result } = renderHook(() =>
      useAutoLayout({
        pipeline: null,
        selectedSessionId: 'sess',
        nodesLength: 0,
        setNodes,
        rf: ref,
        updateNodePosition,
      })
    );

    act(() => {
      result.current.applyAutoLayout({});
    });

    expect(setNodes).not.toHaveBeenCalled();
    expect(updateNodePosition).not.toHaveBeenCalled();
  });

  it('falls back to ESTIMATED_HEIGHT_BY_KIND when no measured height is provided', () => {
    const setNodes = vi.fn();
    const updateNodePosition = vi.fn();
    const { ref } = makeStubInstance();

    const pipeline: Pipeline = {
      ...makePipeline(),
      nodes: {
        src: { kind: 'video::compositor', params: {}, state: null },
        sink: { kind: 'audio::null_sink', params: {}, state: null },
      },
    };

    setNodes.mockImplementation((updater: (prev: RFNode[]) => RFNode[]) =>
      updater([makeNode('src'), makeNode('sink')])
    );

    const { result } = renderHook(() =>
      useAutoLayout({
        pipeline,
        selectedSessionId: 'sess',
        nodesLength: 2,
        setNodes,
        rf: ref,
        updateNodePosition,
      })
    );

    act(() => {
      // No measured height for "src" — falls back to the compositor's
      // estimated height (900px), which pushes "sink" well below.
      result.current.applyAutoLayout({});
    });

    const sinkCall = updateNodePosition.mock.calls.find((c) => c[1] === 'sink');
    expect(sinkCall).toBeDefined();
    expect((sinkCall![2] as { y: number }).y).toBeGreaterThanOrEqual(900);
  });

  it('schedules fitView ~100ms after applying layout (cancels prior timer)', () => {
    const setNodes = vi.fn();
    const { ref, instance } = makeStubInstance();
    setNodes.mockImplementation((updater: (prev: RFNode[]) => RFNode[]) =>
      updater([makeNode('src'), makeNode('sink')])
    );

    const { result } = renderHook(() =>
      useAutoLayout({
        pipeline: makePipeline(),
        selectedSessionId: 'sess',
        nodesLength: 2,
        setNodes,
        rf: ref,
        updateNodePosition: vi.fn(),
      })
    );

    act(() => {
      result.current.applyAutoLayout({ src: 100, sink: 100 });
    });
    expect(instance.fitView).not.toHaveBeenCalled();

    act(() => {
      // Re-invoke before the 100ms timer fires: only one fitView should
      // ever land thanks to the timer cancellation.
      result.current.applyAutoLayout({ src: 100, sink: 100 });
      vi.advanceTimersByTime(100);
    });

    expect(instance.fitView).toHaveBeenCalledTimes(1);
    expect(instance.fitView).toHaveBeenCalledWith({ padding: 0.2, duration: 0 });
  });
});

describe('useAutoLayout — handleAutoLayout', () => {
  it('collects heights from instance.getNodes() and delegates to applyAutoLayout', () => {
    const setNodes = vi.fn();
    const updateNodePosition = vi.fn();
    const nodes = [makeNode('src', 80), makeNode('sink', 200)];
    const { ref, instance } = makeStubInstance(nodes);

    setNodes.mockImplementation((updater: (prev: RFNode[]) => RFNode[]) => updater(nodes));

    const { result } = renderHook(() =>
      useAutoLayout({
        pipeline: makePipeline(),
        selectedSessionId: 'sess',
        nodesLength: 2,
        setNodes,
        rf: ref,
        updateNodePosition,
      })
    );

    act(() => {
      result.current.handleAutoLayout();
      // rAF in happy-dom routes through the fake timers' task queue.
      vi.runAllTimers();
    });

    expect(instance.getNodes).toHaveBeenCalled();
    expect(updateNodePosition).toHaveBeenCalled();
  });

  it('is a no-op when pipeline is null', () => {
    const setNodes = vi.fn();
    const { ref, instance } = makeStubInstance([makeNode('src')]);

    const { result } = renderHook(() =>
      useAutoLayout({
        pipeline: null,
        selectedSessionId: 'sess',
        nodesLength: 0,
        setNodes,
        rf: ref,
        updateNodePosition: vi.fn(),
      })
    );

    act(() => {
      result.current.handleAutoLayout();
      vi.runAllTimers();
    });

    expect(instance.getNodes).not.toHaveBeenCalled();
    expect(setNodes).not.toHaveBeenCalled();
  });
});

describe('useAutoLayout — flag-driven effects', () => {
  it('runs auto-layout once when needsAutoLayout flips to true', () => {
    const setNodes = vi.fn();
    const updateNodePosition = vi.fn();
    const { ref, instance } = makeStubInstance([makeNode('src'), makeNode('sink')]);
    setNodes.mockImplementation((updater: (prev: RFNode[]) => RFNode[]) =>
      updater([makeNode('src'), makeNode('sink')])
    );

    const { result } = renderHook(() =>
      useAutoLayout({
        pipeline: makePipeline(),
        selectedSessionId: 'sess',
        nodesLength: 2,
        setNodes,
        rf: ref,
        updateNodePosition,
      })
    );

    act(() => {
      result.current.setNeedsAutoLayout(true);
      result.current.setNeedsFit(true);
    });

    act(() => {
      // requestIdleCallback / setTimeout-based scheduling — flush.
      vi.advanceTimersByTime(300);
    });

    expect(updateNodePosition).toHaveBeenCalled();
    // The hook should have cleared both flags itself.
    expect(result.current.needsAutoLayout).toBe(false);
    expect(result.current.needsFit).toBe(false);

    // fitView from applyAutoLayout's 100ms timer fires after the layout.
    act(() => {
      vi.advanceTimersByTime(150);
    });
    expect(instance.fitView).toHaveBeenCalled();
  });

  it('does NOT trigger auto-layout when nodesLength is 0', () => {
    const setNodes = vi.fn();
    const updateNodePosition = vi.fn();
    const { ref } = makeStubInstance();

    const { result } = renderHook(() =>
      useAutoLayout({
        pipeline: makePipeline(),
        selectedSessionId: 'sess',
        nodesLength: 0,
        setNodes,
        rf: ref,
        updateNodePosition,
      })
    );

    act(() => {
      result.current.setNeedsAutoLayout(true);
      vi.advanceTimersByTime(500);
    });

    expect(updateNodePosition).not.toHaveBeenCalled();
    // Effect didn't run, so the flag stays true.
    expect(result.current.needsAutoLayout).toBe(true);
  });

  it('fits the view ~150ms after needsFit when auto-layout is NOT active', () => {
    const setNodes = vi.fn();
    const { ref, instance } = makeStubInstance([makeNode('src'), makeNode('sink')]);

    const { result } = renderHook(() =>
      useAutoLayout({
        pipeline: makePipeline(),
        selectedSessionId: 'sess',
        nodesLength: 2,
        setNodes,
        rf: ref,
        updateNodePosition: vi.fn(),
      })
    );

    act(() => {
      result.current.setNeedsFit(true);
    });
    act(() => {
      vi.advanceTimersByTime(150);
    });

    expect(instance.fitView).toHaveBeenCalledWith({ padding: 0.2, duration: 0 });
    expect(result.current.needsFit).toBe(false);
  });

  it('cancels the pending fitView timer on unmount', () => {
    const setNodes = vi.fn();
    const { ref, instance } = makeStubInstance([makeNode('src'), makeNode('sink')]);
    setNodes.mockImplementation((updater: (prev: RFNode[]) => RFNode[]) =>
      updater([makeNode('src'), makeNode('sink')])
    );

    const { result, unmount } = renderHook(() =>
      useAutoLayout({
        pipeline: makePipeline(),
        selectedSessionId: 'sess',
        nodesLength: 2,
        setNodes,
        rf: ref,
        updateNodePosition: vi.fn(),
      })
    );

    act(() => {
      result.current.applyAutoLayout({ src: 100, sink: 100 });
    });
    unmount();
    act(() => {
      vi.advanceTimersByTime(500);
    });

    expect(instance.fitView).not.toHaveBeenCalled();
  });
});
