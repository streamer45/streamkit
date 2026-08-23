// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { Node, Edge, Connection, OnConnectEnd, ReactFlowInstance } from '@xyflow/react';
import { addEdge, MarkerType } from '@xyflow/react';
import { useCallback, useRef } from 'react';

import { useToast } from '@/context/ToastContext';
import type { InputPin, OutputPin } from '@/types/types';
import { wouldCreateCycle } from '@/utils/dag';
import { canConnect, resolveOutputType } from '@/utils/packetTypes';

function isBidirectionalNode(node: Node | undefined): boolean {
  if (!node) return false;
  return (
    ((node.data as Record<string, unknown>).definition as { bidirectional?: boolean })
      ?.bidirectional ?? false
  );
}

function getBidirectionalNodeIds(nodes: Node[]): string[] {
  return nodes.filter(isBidirectionalNode).map((n) => n.id);
}

function checkWouldCreateCycle(
  connection: { source: string; target: string },
  nodes: Node[],
  edges: Edge[]
): boolean {
  const nodeIds = nodes.map((n) => n.id);
  const existingEdges = edges.map((e) => ({ source: e.source, target: e.target }));
  const newEdge = { source: connection.source, target: connection.target };
  const bidirectionalNodeIds = getBidirectionalNodeIds(nodes);
  return wouldCreateCycle(nodeIds, existingEdges, newEdge, bidirectionalNodeIds);
}

function hasExistingConnection(
  targetNodeId: string,
  targetHandle: string | null | undefined,
  edges: Edge[]
): boolean {
  return edges.some((e) => e.target === targetNodeId && e.targetHandle === targetHandle);
}

function getNodePins(node: Node | undefined): { inputs: InputPin[]; outputs: OutputPin[] } {
  if (!node) return { inputs: [], outputs: [] };
  return {
    inputs: ((node.data as Record<string, unknown>).inputs || []) as InputPin[],
    outputs: ((node.data as Record<string, unknown>).outputs || []) as OutputPin[],
  };
}

function validateGhostPinConnection(
  sourceNode: Node | undefined,
  sourceHandle: string | null | undefined,
  targetNode: Node | undefined,
  nodes: Node[],
  edges: Edge[]
): boolean {
  if (!sourceNode || !targetNode) return false;

  const nodeDefinition = (targetNode?.data as Record<string, unknown>)?.nodeDefinition as
    { inputs?: InputPin[] } | undefined;
  const dynamicPinTemplate = nodeDefinition?.inputs?.find(
    (pin) => typeof pin.cardinality === 'object' && 'Dynamic' in pin.cardinality
  );

  if (!dynamicPinTemplate) return false;

  const resolvedType = resolveOutputType(sourceNode, sourceHandle || null, nodes, edges);
  return canConnect(resolvedType, dynamicPinTemplate.accepts_types);
}

function validateNormalPinConnection(
  sourceNode: Node | undefined,
  sourceOutput: OutputPin | undefined,
  targetInput: InputPin | undefined,
  connection: Connection | Edge,
  nodes: Node[],
  edges?: Edge[]
): boolean {
  if (!sourceNode || !sourceOutput || !targetInput) return false;

  if (
    targetInput.cardinality === 'One' &&
    edges &&
    hasExistingConnection(connection.target!, connection.targetHandle, edges)
  ) {
    return false;
  }

  const resolvedType = resolveOutputType(
    sourceNode,
    (connection.sourceHandle || null) as string | null,
    nodes,
    edges ?? []
  );
  return canConnect(resolvedType, targetInput.accepts_types);
}

function isValidBidirectionalSelfLoop(connection: Connection, nodes: Node[]): boolean {
  if (connection.source !== connection.target) return false;
  const sourceNode = nodes.find((n) => n.id === connection.source);
  return (
    ((sourceNode?.data as Record<string, unknown>)?.definition as { bidirectional?: boolean })
      ?.bidirectional ?? false
  );
}

function generateDynamicPin<T extends Record<string, unknown>>(
  targetNode: Node<T>,
  nodeDefinition: { inputs?: InputPin[] } | undefined
): { pinName: string; pin: InputPin } {
  const existingInputs = (targetNode.data.inputs || []) as InputPin[];
  const realInputs = existingInputs.filter(
    (pin) => !(typeof pin.cardinality === 'object' && 'Dynamic' in pin.cardinality)
  );
  const newPinIndex = realInputs.length;
  const pinName = `in_${newPinIndex}`;

  const dynamicPinTemplate = nodeDefinition?.inputs?.find(
    (pin) => typeof pin.cardinality === 'object' && 'Dynamic' in pin.cardinality
  );

  const pin: InputPin = {
    name: pinName,
    accepts_types: dynamicPinTemplate?.accepts_types || ['Any' as never],
    cardinality: 'One',
  };

  return { pinName, pin };
}

function addDynamicPinToNode<T extends Record<string, unknown>>(
  nodes: Node<T>[],
  targetNodeId: string,
  newPin: InputPin
): Node<T>[] {
  return nodes.map((n) => {
    if (n.id === targetNodeId) {
      const existingInputs = (n.data.inputs || []) as InputPin[];
      return {
        ...n,
        data: {
          ...n.data,
          inputs: [...existingInputs, newPin],
        },
      };
    }
    return n;
  });
}

function styleLoopbackEdge(edge: Edge): void {
  edge.style = {
    ...edge.style,
    strokeDasharray: '5, 5',
    stroke: 'var(--sk-text-muted)',
    strokeWidth: 2,
  };
  edge.animated = false;
  edge.markerEnd = {
    type: MarkerType.ArrowClosed,
    color: 'var(--sk-text-muted)',
  };
  edge.data = { ...edge.data, isLoopback: true };
}

function validateConnectionWithToast(
  connection: Connection,
  nodes: Node[],
  edges: Edge[] | undefined,
  isValidConnection: (connection: Connection, nodes: Node[], edges?: Edge[]) => boolean,
  toast: { error: (message: string) => void }
): boolean {
  // Self-loops are only valid on bidirectional nodes
  if (connection.source === connection.target && !isValidBidirectionalSelfLoop(connection, nodes)) {
    toast.error('Cannot connect: self-loops are only allowed for bidirectional nodes');
    return false;
  }

  if (connection.source !== connection.target && !isValidConnection(connection, nodes, edges)) {
    if (edges && checkWouldCreateCycle(connection, nodes, edges)) {
      toast.error('Cannot connect: this would create a cycle in the pipeline');
      return false;
    }

    const targetNode = nodes.find((n) => n.id === connection.target);
    if (targetNode && edges) {
      const { inputs: targetInputs } = getNodePins(targetNode);
      const targetInput = targetInputs.find(
        (i: InputPin) => i.name === (connection.targetHandle || 'in')
      );

      if (
        targetInput?.cardinality === 'One' &&
        hasExistingConnection(connection.target!, connection.targetHandle, edges)
      ) {
        toast.error('Cannot connect: this input pin already has a connection (cardinality: 1-1)');
        return false;
      }
    }

    toast.error('Cannot connect: incompatible packet types');
    return false;
  }

  return true;
}

function handleDynamicPinCreation<T extends Record<string, unknown>>(
  connection: Connection,
  nodes: Node<T>[],
  setNodes: ((updater: (nodes: Node<T>[]) => Node<T>[]) => void) | undefined
): Connection {
  const targetIsGhost = connection.targetHandle?.startsWith('__ghost__');

  if (targetIsGhost && setNodes) {
    const targetNode = nodes.find((n) => n.id === connection.target);
    if (targetNode) {
      const nodeDefinition = (targetNode.data as Record<string, unknown>).nodeDefinition as
        { inputs?: InputPin[] } | undefined;
      const { pinName, pin } = generateDynamicPin(targetNode, nodeDefinition);

      setNodes((nds) => addDynamicPinToNode(nds, connection.target!, pin));
      return {
        ...connection,
        targetHandle: pinName,
      };
    }
  }

  return connection;
}

function addEdgeWithMetadata(connection: Connection, edges: Edge[], nodes: Node[]): Edge[] {
  const newEdges = addEdge(connection, edges);
  const addedEdge = newEdges[newEdges.length - 1];

  if (addedEdge) {
    // Handles Passthrough type inference
    const sourceNode = nodes.find((n) => n.id === connection.source);
    if (sourceNode) {
      const resolvedType = resolveOutputType(sourceNode, connection.sourceHandle, nodes, edges);
      addedEdge.data = { ...addedEdge.data, resolvedType };
    }

    if (connection.source === connection.target) {
      styleLoopbackEdge(addedEdge);
    }
  }

  return newEdges;
}

function getConnectionErrorMessage(
  connection: {
    source: string;
    target: string;
    sourceHandle: string | null;
    targetHandle: string | null;
  },
  nodes: Node[],
  edges: Edge[]
): string | null {
  if (checkWouldCreateCycle(connection, nodes, edges)) {
    return 'Cannot connect: this would create a cycle in the pipeline';
  }

  const sourceNode = nodes.find((n) => n.id === connection.source);
  const targetNode = nodes.find((n) => n.id === connection.target);

  if (!sourceNode || !targetNode) {
    return null;
  }

  const { outputs: sourceOutputs } = getNodePins(sourceNode);
  const sourceOutput = sourceOutputs.find(
    (o: OutputPin) => o.name === (connection.sourceHandle || 'out')
  );
  const { inputs: targetInputs } = getNodePins(targetNode);
  const targetInput = targetInputs.find(
    (i: InputPin) => i.name === (connection.targetHandle || 'in')
  );

  // Check if pins exist (might be connecting in wrong direction)
  if (!sourceOutput || !targetInput) {
    return 'Cannot connect: connection must go from output pin to input pin';
  }

  if (
    targetInput.cardinality === 'One' &&
    hasExistingConnection(connection.target, connection.targetHandle, edges)
  ) {
    return 'Cannot connect: this input pin already has a connection (cardinality: 1-1)';
  }

  const resolvedType = resolveOutputType(sourceNode, connection.sourceHandle, nodes, edges);
  if (!canConnect(resolvedType, targetInput.accepts_types)) {
    return 'Cannot connect: incompatible packet types';
  }

  return 'Cannot connect: invalid connection';
}

export function useReactFlowCommon() {
  const rf = useRef<ReactFlowInstance | null>(null);
  const toast = useToast();

  const onInit = useCallback((instance: ReactFlowInstance) => {
    rf.current = instance;
  }, []);

  const screenToFlow = useCallback((pt: { x: number; y: number }) => {
    return rf.current?.screenToFlowPosition(pt) ?? pt;
  }, []);

  const isValidConnection = useCallback(
    (connection: Connection | Edge, nodes: Node[], edges?: Edge[]) => {
      const sourceNode = nodes.find((n) => n.id === connection.source);
      const targetNode = nodes.find((n) => n.id === connection.target);
      if (!sourceNode || !targetNode) {
        return true;
      }

      // Allow self-loops for bidirectional nodes
      const isSelfLoop = connection.source === connection.target;
      if (isSelfLoop && isBidirectionalNode(sourceNode)) {
        return true;
      }

      if (edges && checkWouldCreateCycle(connection, nodes, edges)) {
        return false;
      }

      const { outputs: sourceOutputs } = getNodePins(sourceNode);
      const sourceOutput = sourceOutputs.find(
        (o: OutputPin) => o.name === (connection.sourceHandle || 'out')
      );
      const { inputs: targetInputs } = getNodePins(targetNode);
      const targetInput = targetInputs.find(
        (i: InputPin) => i.name === (connection.targetHandle || 'in')
      );

      const sourceIsGhost = connection.sourceHandle?.startsWith('__ghost__');
      const targetIsGhost = connection.targetHandle?.startsWith('__ghost__');

      // Ghost output pins are not yet supported (only ghost inputs)
      if (sourceIsGhost) {
        return false;
      }

      if (targetIsGhost) {
        return validateGhostPinConnection(
          sourceNode,
          connection.sourceHandle,
          targetNode,
          nodes,
          edges ?? []
        );
      }

      return validateNormalPinConnection(
        sourceNode,
        sourceOutput,
        targetInput,
        connection,
        nodes,
        edges
      );
    },
    []
  );

  const createOnConnect = useCallback(
    <T extends Record<string, unknown> = Record<string, unknown>>(
      nodes: Node<T>[],
      setEdges: (updater: (edges: Edge[]) => Edge[]) => void,
      onConnectCallback?: (connection: Connection) => void,
      edges?: Edge[],
      setNodes?: (updater: (nodes: Node<T>[]) => Node<T>[]) => void
    ) => {
      return (connection: Connection) => {
        if (!validateConnectionWithToast(connection, nodes, edges, isValidConnection, toast)) {
          return;
        }

        const finalConnection = handleDynamicPinCreation(connection, nodes, setNodes);

        if (onConnectCallback) {
          onConnectCallback(finalConnection);
        } else {
          setEdges((eds) => addEdgeWithMetadata(finalConnection, eds, nodes));
        }
      };
    },
    [isValidConnection, toast]
  );

  const createOnConnectEnd = useCallback(
    (nodes: Node[], edges: Edge[]): OnConnectEnd => {
      return (_event, connectionState) => {
        if (!connectionState.isValid && connectionState.fromNode && connectionState.toNode) {
          const connection = {
            source: connectionState.fromNode.id,
            target: connectionState.toNode.id,
            sourceHandle: connectionState.fromHandle?.id || null,
            targetHandle: connectionState.toHandle?.id || null,
          };

          const errorMessage = getConnectionErrorMessage(connection, nodes, edges);
          if (errorMessage) {
            toast.error(errorMessage);
          }
        }
      };
    },
    [toast]
  );

  return {
    rf,
    onInit,
    screenToFlow,
    isValidConnection,
    createOnConnect,
    createOnConnectEnd,
  };
}
