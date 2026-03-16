// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Node interaction callbacks for the Monitor view: drag/drop, connect, delete.
 *
 * Extracted from MonitorViewContent to keep node-action concerns self-contained
 * and reduce the component's statement count.
 */

import type {
  Node as RFNode,
  Edge,
  Connection as RFConnection,
  ReactFlowInstance,
  OnConnectEnd,
} from '@xyflow/react';
import React, { useCallback } from 'react';

import { useDnD } from '@/context/DnDContext';
import { useSchemaStore } from '@/stores/schemaStore';
import { useStagingStore } from '@/stores/stagingStore';
import type { Connection, Node as PipelineNode, Pipeline } from '@/types/types';
import { viewsLogger } from '@/utils/logger';

interface UseMonitorNodeActionsOptions {
  selectedSessionId: string | null;
  pipelineRef: React.RefObject<Pipeline | null>;
  isInStagingMode: boolean;
  nodesRefForCallbacks: React.RefObject<RFNode[]>;
  edgesRefForCallbacks: React.RefObject<Edge[]>;
  setNodes: React.Dispatch<React.SetStateAction<RFNode[]>>;
  setEdges: React.Dispatch<React.SetStateAction<Edge[]>>;
  createOnConnect: (
    nodes: RFNode[],
    setEdges: (updater: (edges: Edge[]) => Edge[]) => void,
    onConnectCallback?: (connection: RFConnection) => void,
    edges?: Edge[],
    setNodes?: (updater: (nodes: RFNode[]) => RFNode[]) => void
  ) => (connection: RFConnection) => void;
  createOnConnectEnd: (nodes: RFNode[], edges: Edge[]) => OnConnectEnd;
  addStagedConnection: (sessionId: string, connection: Connection) => void;
  removeStagedConnection: (sessionId: string, connection: Connection) => void;
  addStagedNode: (sessionId: string, nodeId: string, node: PipelineNode) => void;
  removeStagedNode: (sessionId: string, nodeId: string) => void;
  connectPins: (fromNode: string, fromPin: string, toNode: string, toPin: string) => void;
  disconnectPins: (fromNode: string, fromPin: string, toNode: string, toPin: string) => void;
  addNode: (nodeId: string, kind: string, params: Record<string, unknown>) => void;
  removeNode: (nodeId: string) => void;
  pendingNodePositions: React.MutableRefObject<Map<string, { x: number; y: number }>>;
  rfInstance: React.RefObject<ReactFlowInstance | null>;
}

export function useMonitorNodeActions({
  selectedSessionId,
  pipelineRef,
  isInStagingMode,
  nodesRefForCallbacks,
  edgesRefForCallbacks,
  setNodes,
  setEdges,
  createOnConnect,
  createOnConnectEnd,
  addStagedConnection,
  removeStagedConnection,
  addStagedNode,
  removeStagedNode,
  connectPins,
  disconnectPins,
  addNode,
  removeNode,
  pendingNodePositions,
  rfInstance,
}: UseMonitorNodeActionsOptions) {
  const [type, setType] = useDnD();

  // ── Name generation ───────────────────────────────────────────────────
  const generateName = useCallback(
    (kind: string) => {
      const pipeline = pipelineRef.current;
      const existing = pipeline ? Object.keys(pipeline.nodes) : [];
      let i = 1;
      let candidate = `${kind}_${i}`;
      while (existing.includes(candidate)) {
        i += 1;
        candidate = `${kind}_${i}`;
      }
      return candidate;
    },
    [pipelineRef]
  );

  // ── Default parameters ────────────────────────────────────────────────
  const getDefaultParams = useCallback((kind: string): Record<string, unknown> => {
    const { nodeDefinitions } = useSchemaStore.getState();
    const def = nodeDefinitions.find((d) => d.kind === kind);
    const params: Record<string, unknown> = {};
    const schema = def?.param_schema as Record<string, unknown> | undefined;
    const props = schema?.properties as Record<string, Record<string, unknown>> | undefined;
    if (props) {
      Object.entries(props).forEach(([key, propSchema]) => {
        if (propSchema && typeof propSchema === 'object' && 'default' in propSchema) {
          const defVal = propSchema.default;
          if (defVal !== undefined) {
            params[key] = defVal;
          }
        }
      });
    }
    return params;
  }, []);

  // ── Connection handlers ───────────────────────────────────────────────
  const onConnect = useCallback(
    (connection: RFConnection) => {
      return createOnConnect(
        nodesRefForCallbacks.current,
        setEdges,
        (conn: RFConnection) => {
          const from_pin = conn.sourceHandle || 'out';
          const to_pin = conn.targetHandle || 'in';

          if (isInStagingMode && selectedSessionId) {
            // Add to staging store instead of sending to server
            addStagedConnection(selectedSessionId, {
              from_node: conn.source,
              from_pin,
              to_node: conn.target,
              to_pin,
            });
          } else {
            // Send to server immediately (monitor mode)
            connectPins(conn.source, from_pin, conn.target, to_pin);
          }
        },
        edgesRefForCallbacks.current,
        setNodes
      )(connection);
    },
    [
      createOnConnect,
      setEdges,
      isInStagingMode,
      selectedSessionId,
      addStagedConnection,
      connectPins,
      setNodes,
    ]
  );

  const onConnectEnd: OnConnectEnd = useCallback(
    (event, connectionState) => {
      return createOnConnectEnd(nodesRefForCallbacks.current, edgesRefForCallbacks.current)(
        event,
        connectionState
      );
    },
    [createOnConnectEnd]
  );

  // ── Deletion handlers ─────────────────────────────────────────────────
  const onEdgesDelete = useCallback(
    (deleted: Edge[]) => {
      const sid = selectedSessionId;
      const staging = sid ? useStagingStore.getState().staging[sid] : undefined;
      const isStagingActive = staging?.mode === 'staging';

      deleted.forEach((e) => {
        const from_pin = e.sourceHandle || 'out';
        const to_pin = e.targetHandle || 'in';

        if (isStagingActive && sid) {
          removeStagedConnection(sid, {
            from_node: e.source,
            from_pin,
            to_node: e.target,
            to_pin,
          });
        } else {
          disconnectPins(e.source, from_pin, e.target, to_pin);
        }
      });
    },
    [selectedSessionId, removeStagedConnection, disconnectPins]
  );

  const onNodesDelete = useCallback(
    (deleted: RFNode[]) => {
      const sid = selectedSessionId;
      const staging = sid ? useStagingStore.getState().staging[sid] : undefined;
      const isStagingActive = staging?.mode === 'staging';

      deleted.forEach((n) => {
        if (isStagingActive && sid) {
          removeStagedNode(sid, n.id);
        } else {
          removeNode(n.id);
        }
      });
    },
    [selectedSessionId, removeStagedNode, removeNode]
  );

  // ── Context menu handlers ─────────────────────────────────────────────
  const handleDuplicateNode = useCallback((nodeId: string) => {
    // In monitor mode, we could potentially duplicate via WebSocket
    viewsLogger.debug('Duplicate node:', nodeId);
  }, []);

  const handleDeleteNode = useCallback(
    (nodeId: string) => {
      removeNode(nodeId);
    },
    [removeNode]
  );

  // ── Drag & drop handlers ──────────────────────────────────────────────
  const onDragStart = useCallback(
    (event: React.DragEvent, nodeType: string) => {
      setType(nodeType);
      event.dataTransfer.setData('text/plain', nodeType);
      event.dataTransfer.effectAllowed = 'move';
    },
    [setType]
  );

  const onDragOver = useCallback((event: React.DragEvent) => {
    event.preventDefault();
    event.dataTransfer.dropEffect = 'move';
  }, []);

  const onDrop = useCallback(
    (event: React.DragEvent) => {
      event.preventDefault();
      if (!type) {
        return;
      }

      // Calculate drop position in flow coordinates
      const position = rfInstance.current?.screenToFlowPosition({
        x: event.clientX,
        y: event.clientY,
      }) ?? { x: event.clientX, y: event.clientY };

      const kind = type;
      const nodeId = generateName(kind);
      const params = getDefaultParams(kind);

      // Cache the position for when the node appears in the pipeline
      pendingNodePositions.current.set(nodeId, position);

      const sid = selectedSessionId;
      const staging = sid ? useStagingStore.getState().staging[sid] : undefined;
      const isStagingActive = staging?.mode === 'staging';

      if (isStagingActive && sid) {
        // Add to staging store
        addStagedNode(sid, nodeId, {
          kind,
          params: params as Record<string, unknown>,
          state: null,
        });
      } else {
        // Send to server immediately (monitor mode)
        addNode(nodeId, kind, params);
      }

      setType(null);
    },
    [type, selectedSessionId, addStagedNode, addNode, setType, generateName, getDefaultParams]
  );

  return {
    onConnect,
    onConnectEnd,
    onEdgesDelete,
    onNodesDelete,
    onDragStart,
    onDrop,
    onDragOver,
    handleDuplicateNode,
    handleDeleteNode,
  };
}
