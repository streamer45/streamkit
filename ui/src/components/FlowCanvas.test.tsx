// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { render, act, waitFor } from '@testing-library/react';
import {
  ReactFlowProvider,
  useNodesState,
  useStoreApi,
  type Node,
  type OnSelectionChangeParams,
} from '@xyflow/react';
import { useEffect, useRef } from 'react';
import { describe, it, expect, vi } from 'vitest';

import { nodeTypes, defaultEdgeOptions } from '@/utils/reactFlowDefaults';

import { FlowCanvas } from './FlowCanvas';

// FlowCanvas statically imports the node-component registry and the edge/connection-line
// components for its prop types, none of which this test exercises — selection is driven
// through the xyflow store. Stub them so the test stays a focused unit on selection
// forwarding instead of mounting the (separately-owned) node subtree.
vi.mock('@/nodes/AudioGainNode', () => ({ default: () => null }));
vi.mock('@/nodes/CompositorNode', () => ({ default: () => null }));
vi.mock('@/nodes/ConfigurableNode', () => ({ default: () => null }));
vi.mock('@/components/TypedEdge', () => ({ default: () => null }));
vi.mock('./ConnectionLine', () => ({ default: () => null }));

if (!('ResizeObserver' in globalThis)) {
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
}

const initialNodes: Node[] = [
  {
    id: 'a',
    type: 'configurable',
    position: { x: 0, y: 0 },
    data: { label: 'A', kind: 'core', params: {} },
  },
  {
    id: 'b',
    type: 'configurable',
    position: { x: 0, y: 120 },
    data: { label: 'B', kind: 'core', params: {} },
  },
];

let store: ReturnType<typeof useStoreApi> | null = null;

function StoreProbe() {
  const api = useStoreApi();
  useEffect(() => {
    store = api;
  }, [api]);
  return null;
}

function Harness({
  showCanvas,
  onSelectionChange,
}: {
  showCanvas: boolean;
  onSelectionChange: (params: OnSelectionChangeParams) => void;
}) {
  const [nodes, , onNodesChange] = useNodesState(initialNodes);
  const wrapperRef = useRef<HTMLDivElement | null>(null);
  return (
    <ReactFlowProvider>
      <StoreProbe />
      {showCanvas ? (
        <FlowCanvas
          nodes={nodes}
          edges={[]}
          nodeTypes={nodeTypes}
          onNodesChange={onNodesChange}
          onEdgesChange={() => {}}
          colorMode="light"
          onInit={() => {}}
          defaultEdgeOptions={defaultEdgeOptions}
          editMode
          onSelectionChange={onSelectionChange}
          onPaneClick={() => {}}
          onPaneContextMenu={() => {}}
          onNodeContextMenu={() => {}}
          reactFlowWrapper={wrapperRef}
        />
      ) : null}
    </ReactFlowProvider>
  );
}

function selectedIds(onSelectionChange: ReturnType<typeof vi.fn>): string[] {
  const params = onSelectionChange.mock.calls.at(-1)?.[0] as OnSelectionChangeParams | undefined;
  return (params?.nodes ?? []).map((n) => n.id);
}

describe('FlowCanvas onSelectionChange', () => {
  // MonitorView renders the canvas conditionally on async session data, so <ReactFlow>
  // unmounts and remounts after mount. xyflow resets its store on that unmount, which
  // dropped the useOnSelectionChange registration; the onSelectionChange prop is re-applied
  // on every render and must keep firing across the remount.
  it('keeps notifying selection after the canvas remounts', async () => {
    const onSelectionChange = vi.fn();
    const { rerender } = render(<Harness showCanvas onSelectionChange={onSelectionChange} />);
    await waitFor(() => expect(store).not.toBeNull());

    act(() => store!.getState().addSelectedNodes(['a']));
    await waitFor(() => expect(selectedIds(onSelectionChange)).toEqual(['a']));

    rerender(<Harness showCanvas={false} onSelectionChange={onSelectionChange} />);
    rerender(<Harness showCanvas onSelectionChange={onSelectionChange} />);
    onSelectionChange.mockClear();

    act(() => store!.getState().addSelectedNodes(['b']));
    await waitFor(() => expect(selectedIds(onSelectionChange)).toEqual(['b']));
  });
});
