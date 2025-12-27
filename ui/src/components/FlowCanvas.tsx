// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type {
  Node,
  Edge,
  Connection,
  OnNodesChange,
  OnEdgesChange,
  OnConnectEnd,
  ReactFlowInstance,
} from '@xyflow/react';
import { ReactFlow, Background, Controls, MarkerType } from '@xyflow/react';
import React from 'react';

import TypedEdge from '@/components/TypedEdge';
import AudioGainNode from '@/nodes/AudioGainNode';
import ConfigurableNode from '@/nodes/ConfigurableNode';

import ConnectionLine from './ConnectionLine';

const nodeOrigin = [0.5, 0] as [number, number];

const edgeTypes = {
  typed: TypedEdge,
};

// Helper to compute ReactFlow props based on edit mode and selection mode
function computeReactFlowProps<NodeData extends Record<string, unknown>>(
  editMode: boolean,
  selectionMode: boolean | undefined,
  callbacks: {
    onConnect?: (conn: Connection) => void;
    onConnectEnd?: OnConnectEnd;
    onDrop?: (event: React.DragEvent) => void;
    onDragOver?: (event: React.DragEvent) => void;
    onNodeDragStop?: (
      event: React.MouseEvent,
      node: Node<NodeData>,
      nodes: Node<NodeData>[]
    ) => void;
    isValidConnection?: (conn: Connection | Edge) => boolean;
    onPaneContextMenu: (event: React.MouseEvent | MouseEvent) => void;
    onNodeContextMenu: (event: React.MouseEvent, node: Node<NodeData>) => void;
    onEdgesDelete?: (edges: Edge[]) => void;
    onNodesDelete?: (nodes: Node<NodeData>[]) => void;
  }
) {
  const isSelectable = editMode || selectionMode;

  return {
    onConnect: editMode ? callbacks.onConnect : undefined,
    onConnectEnd: editMode ? callbacks.onConnectEnd : undefined,
    onDrop: editMode ? callbacks.onDrop : undefined,
    onDragOver: editMode ? callbacks.onDragOver : undefined,
    onNodeDragStop: editMode ? callbacks.onNodeDragStop : undefined,
    isValidConnection: editMode ? callbacks.isValidConnection : undefined,
    onPaneContextMenu: isSelectable ? callbacks.onPaneContextMenu : undefined,
    onNodeContextMenu: isSelectable ? callbacks.onNodeContextMenu : undefined,
    onEdgesDelete: isSelectable ? callbacks.onEdgesDelete : undefined,
    onNodesDelete: isSelectable ? callbacks.onNodesDelete : undefined,
    selectionOnDrag: isSelectable,
    panOnDrag: !selectionMode,
    nodesDraggable: isSelectable,
    nodesConnectable: editMode,
    elementsSelectable: isSelectable,
    deleteKeyCode: (isSelectable ? ['Delete'] : undefined) as string[] | undefined,
  };
}

export const FlowCanvas = <NodeData extends Record<string, unknown> = Record<string, unknown>>({
  nodes,
  edges,
  nodeTypes,
  onNodesChange,
  onEdgesChange,
  colorMode,
  onInit,
  defaultEdgeOptions,
  editMode,
  selectionMode,
  isValidConnection,
  onConnect,
  onConnectEnd,
  onEdgesDelete,
  onNodesDelete,
  onNodeDoubleClick,
  onPaneClick,
  onPaneContextMenu,
  onNodeContextMenu,
  onNodeDragStop,
  onDrop,
  onDragOver,
  reactFlowWrapper,
}: {
  nodes: Node<NodeData>[];
  edges: Edge[];
  nodeTypes: {
    audioGain: typeof AudioGainNode;
    configurable: typeof ConfigurableNode;
  };
  onNodesChange: OnNodesChange<Node<NodeData>>;
  onEdgesChange: OnEdgesChange;
  colorMode: 'dark' | 'light';
  onInit: (instance: ReactFlowInstance<Node<NodeData>>) => void;
  defaultEdgeOptions: {
    type: string;
    animated: boolean;
    style: {
      stroke: string;
      strokeWidth: number;
    };
    markerEnd?: {
      type: MarkerType;
      width: number;
      height: number;
      color: string;
    };
  };
  editMode: boolean;
  selectionMode?: boolean;
  isValidConnection?: (conn: Connection | Edge) => boolean;
  onConnect?: (conn: Connection) => void;
  onConnectEnd?: OnConnectEnd;
  onEdgesDelete?: (edges: Edge[]) => void;
  onNodesDelete?: (nodes: Node<NodeData>[]) => void;
  onNodeDoubleClick?: (event: React.MouseEvent, node: Node<NodeData>) => void;
  onPaneClick: () => void;
  onPaneContextMenu: (event: React.MouseEvent | MouseEvent) => void;
  onNodeContextMenu: (event: React.MouseEvent, node: Node<NodeData>) => void;
  onNodeDragStop?: (event: React.MouseEvent, node: Node<NodeData>, nodes: Node<NodeData>[]) => void;
  onDrop?: (event: React.DragEvent) => void;
  onDragOver?: (event: React.DragEvent) => void;
  reactFlowWrapper: React.RefObject<HTMLDivElement | null>;
}) => {
  const computedProps = computeReactFlowProps(editMode, selectionMode, {
    onConnect,
    onConnectEnd,
    onDrop,
    onDragOver,
    onNodeDragStop,
    isValidConnection,
    onPaneContextMenu,
    onNodeContextMenu,
    onEdgesDelete,
    onNodesDelete,
  });

  return (
    <div ref={reactFlowWrapper} style={{ width: '100%', height: '100%' }}>
      <ReactFlow<Node<NodeData>>
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        edgeTypes={edgeTypes}
        onNodesChange={onNodesChange}
        onEdgesChange={onEdgesChange}
        onNodeDoubleClick={onNodeDoubleClick}
        onPaneClick={onPaneClick}
        colorMode={colorMode}
        onInit={onInit}
        connectionLineComponent={ConnectionLine}
        nodeOrigin={nodeOrigin}
        defaultEdgeOptions={defaultEdgeOptions}
        nodeDragThreshold={1}
        maxZoom={1.5}
        minZoom={0.25}
        onlyRenderVisibleElements={true}
        {...computedProps}
      >
        <Background />
        <Controls />
      </ReactFlow>
    </div>
  );
};
