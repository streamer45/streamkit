// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import {
  ReactFlowProvider,
  useNodesState,
  useEdgesState,
  useOnSelectionChange,
  type Node as RFNode,
  type Edge,
  type NodeChange,
  type Connection as RFConnection,
  type ReactFlowInstance,
  type OnConnectEnd,
} from '@xyflow/react';
import { dump } from 'js-yaml';
import React, { useState, useEffect, useCallback, useRef } from 'react';
import { useLocation } from 'react-router-dom';
import { v4 as uuidv4 } from 'uuid';
import { useShallow } from 'zustand/shallow';

import ConfirmModal from '@/components/ConfirmModal';
import ContextMenu from '@/components/ContextMenu';
import { FlowCanvas } from '@/components/FlowCanvas';
import { LeftPanel } from '@/components/monitor/LeftPanel';
import { Legend } from '@/components/monitor/Legend';
import {
  CenterPanelContainer,
  CanvasTopBar,
  TopLeftControls,
  EmptyMonitorState,
} from '@/components/monitor/MonitorView.styles';
import { SessionInfoChip } from '@/components/monitor/SessionItem';
import { TopControls } from '@/components/monitor/TopControls';
import { OutputPreviewPanel } from '@/components/OutputPreviewPanel';
import PaneContextMenu from '@/components/PaneContextMenu';
import { PipelineRightPane } from '@/components/PipelineRightPane';
import { ResizableLayout } from '@/components/ResizableLayout';
import { ViewTitle } from '@/components/ui/ViewTitle';
import { DnDProvider, useDnD } from '@/context/DnDContext';
import { useToast } from '@/context/ToastContext';
import { useAutoLayout } from '@/hooks/useAutoLayout';
import { useContextMenu } from '@/hooks/useContextMenu';
import { useMonitorPreview } from '@/hooks/useMonitorPreview';
import { useNodeStatesSubscription } from '@/hooks/useNodeStatesSubscription';
import { useReactFlowCommon } from '@/hooks/useReactFlowCommon';
import { useResolvedColorMode } from '@/hooks/useResolvedColorMode';
import { useSession } from '@/hooks/useSession';
import { useSessionList } from '@/hooks/useSessionList';
import { useTuneNode } from '@/hooks/useTuneNode';
import { useWebSocket } from '@/hooks/useWebSocket';
import { getWebSocketService } from '@/services/websocket';
import { useLayoutStore } from '@/stores/layoutStore';
import { useNodePositionStore } from '@/stores/nodePositionStore';
import { usePluginStore } from '@/stores/pluginStore';
import { useSchemaStore } from '@/stores/schemaStore';
import {
  sessionStore as defaultSessionStore,
  nodeStateAtom,
  nodeParamsAtom,
  nodeKey,
} from '@/stores/sessionAtoms';
import { useSessionStore } from '@/stores/sessionStore';
import type {
  NodeDefinition,
  Connection,
  Pipeline,
  MessageType,
  InputPin,
  OutputPin,
} from '@/types/types';
import { dispatchParamUpdate } from '@/utils/controlProps';
import { topoLevelsFromPipeline, orderedNamesFromLevels } from '@/utils/dag';
import { deepEqual } from '@/utils/deepEqual';
import { deepMergeSchemas, validateValue } from '@/utils/jsonSchema';
import type { JsonSchema } from '@/utils/jsonSchema';
import { viewsLogger } from '@/utils/logger';
import {
  buildEdgesFromConnections,
  buildNodeObject,
  generatePipelineYaml,
} from '@/utils/pipelineGraph';
import { nodeTypes, defaultEdgeOptions } from '@/utils/reactFlowDefaults';

// Memoized view title to prevent re-renders during drag
const MonitorViewTitle = React.memo(() => <ViewTitle>Monitor</ViewTitle>);

/**
 * Main content component for the Monitor view.
 * This component has 114 statements which exceeds the max-statements limit.
 * However, breaking it down would require significant architectural changes
 * and may reduce code cohesion. The complexity is managed through:
 * - Extracted helper functions for complex operations
 * - useCallback/useMemo hooks to optimize re-renders
 * - Clear separation of concerns via comments
 */
// eslint-disable-next-line max-statements -- Main view component with many hooks and state management
const MonitorViewContent: React.FC = () => {
  const location = useLocation();
  const [selectedSessionId, setSelectedSessionId] = useState<string | null>(null);
  const [nodes, setNodes, onNodesChangeInternal] = useNodesState<RFNode>([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>([]);

  // ── Low-priority dimension changes ────────────────────────────────────
  // ReactFlow fires onNodesChange with 'dimensions' type for each node
  // after mount measurement.  These are internal bookkeeping (the nodes
  // are already visible) so we wrap them in startTransition to let React
  // schedule them at lower priority rather than blocking the main thread.
  // Interactive changes (select, drag, remove) bypass this and apply
  // immediately.
  const onNodesChangeBatched = useCallback(
    (changes: NodeChange[]) => {
      const immediate: NodeChange[] = [];
      const deferred: NodeChange[] = [];

      for (const c of changes) {
        if (c.type === 'dimensions') {
          deferred.push(c);
        } else {
          immediate.push(c);
        }
      }

      if (immediate.length > 0) {
        onNodesChangeInternal(immediate);
      }

      if (deferred.length > 0) {
        React.startTransition(() => {
          onNodesChangeInternal(deferred);
        });
      }
    },
    [onNodesChangeInternal]
  );
  const [yamlString, setYamlString] = useState<string>('');
  const nodeDefinitions = useSchemaStore(useShallow((s) => s.nodeDefinitions));
  const plugins = usePluginStore(useShallow((s) => s.plugins));
  const pluginKinds = React.useMemo(() => new Set(plugins.map((p) => p.kind)), [plugins]);
  const pluginTypes = React.useMemo(
    () => new Map(plugins.map((p) => [p.kind, p.plugin_type])),
    [plugins]
  );

  // Node position store for persisting canvas positions
  const updateNodePosition = useNodePositionStore((s) => s.updateNodePosition);
  const getNodePositions = useNodePositionStore((s) => s.getNodePositions);
  const clearSessionPositions = useNodePositionStore((s) => s.clearSession);

  // Save node positions when drag stops
  const onNodeDragStop = useCallback(
    (_event: React.MouseEvent, node: RFNode) => {
      if (selectedSessionId) {
        updateNodePosition(selectedSessionId, node.id, node.position);
      }
    },
    [selectedSessionId, updateNodePosition]
  );
  const [selectedNodes, setSelectedNodes] = useState<string[]>([]);
  const [rightPaneView, setRightPaneView] = useState<'yaml' | 'inspector' | 'telemetry'>('yaml');
  const [showDeleteModal, setShowDeleteModal] = useState(false);
  const [sessionToDelete, setSessionToDelete] = useState<string | null>(null);
  const [isDeletingSession, setIsDeletingSession] = useState(false);
  const colorMode = useResolvedColorMode();
  const { rightCollapsed, setRightCollapsed } = useLayoutStore(
    useShallow((state) => ({
      rightCollapsed: state.rightCollapsed,
      setRightCollapsed: state.setRightCollapsed,
    }))
  );
  const [type, setType] = useDnD();
  const toast = useToast();
  // Cache for positions of nodes that are being added (to preserve drop location)
  const pendingNodePositions = React.useRef<Map<string, { x: number; y: number }>>(new Map());

  // Auto-select session from navigation state (e.g., from Stream view)
  useEffect(() => {
    const state = location.state as { sessionId?: string } | null;
    if (state?.sessionId && !selectedSessionId) {
      const sessionId = state.sessionId;
      setSelectedSessionId(sessionId);

      // Check if this session has saved positions
      const savedPos = getNodePositions(sessionId);
      const hasPositions = Object.keys(savedPos).length > 0;

      // Trigger auto-layout if no positions are saved
      setNeedsAutoLayout(!hasPositions);
      setNeedsFit(true);

      // Clear the state to avoid auto-selecting on subsequent visits
      window.history.replaceState({}, document.title);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- setNeedsAutoLayout/setNeedsFit are stable useState setters declared later
  }, [location.state, selectedSessionId, getNodePositions]);

  // Use shared React Flow logic
  const {
    onInit: baseOnInit,
    isValidConnection,
    createOnConnect,
    createOnConnectEnd,
  } = useReactFlowCommon();
  const rf = React.useRef<ReactFlowInstance | null>(null);
  const onInit = (instance: ReactFlowInstance) => {
    rf.current = instance;
    baseOnInit(instance);
  };
  const screenToFlow = (pt: { x: number; y: number }) => {
    return rf.current?.screenToFlowPosition(pt) ?? pt;
  };

  // Keep refs to avoid recreating callbacks on every drag
  const nodesRefForCallbacks = React.useRef(nodes);
  const edgesRefForCallbacks = React.useRef(edges);
  React.useEffect(() => {
    nodesRefForCallbacks.current = nodes;
    edgesRefForCallbacks.current = edges;
  }, [nodes, edges]);

  // Use shared context menu logic
  const { menu, paneMenu, reactFlowWrapper, onNodeContextMenu, onPaneContextMenu, onPaneClick } =
    useContextMenu();

  useOnSelectionChange({
    onChange: ({ nodes: selNodes }) => {
      const nextIds = selNodes.map((n) => n.id);
      setSelectedNodes((prev) =>
        prev.length === nextIds.length && prev.every((v, i) => v === nextIds[i]) ? prev : nextIds
      );
    },
  });

  // Keep YAML view as default when nodes are selected
  // Inspector only opens on double-click
  useEffect(() => {
    if (selectedNodes.length === 0) {
      // No selection - keep YAML view
      setRightPaneView('yaml');
    } else if (selectedNodes.length > 1) {
      // Multiple selection - show YAML view
      setRightPaneView('yaml');
    } else if (selectedNodes.length === 1) {
      // Single selection - switch to YAML view (with highlighting)
      setRightPaneView('yaml');
    }
  }, [selectedNodes]);

  // Double-click handler to open inspector
  const handleNodeDoubleClick = React.useCallback(() => {
    setRightPaneView('inspector');
    // Expand right pane if collapsed
    if (rightCollapsed) {
      setRightCollapsed(false);
    }
  }, [rightCollapsed, setRightCollapsed]);

  // Memoize selectedNode with custom comparison (ignore position changes)
  const selectedNodeId = selectedNodes.length === 1 ? selectedNodes[0] : null;
  const selectedNode = React.useMemo(() => {
    if (!selectedNodeId) return null;
    return nodes.find((node) => node.id === selectedNodeId) ?? null;
  }, [selectedNodeId, nodes]);

  // Create a stable reference for selectedNode that only changes when data (not position) changes
  const selectedNodeRef = React.useRef(selectedNode);

  // React Flow triggers renders on position changes; we only want to re-render inspector on data changes.
  const stableSelectedNode = React.useMemo(() => {
    if (!selectedNode) {
      selectedNodeRef.current = null;
      return null;
    }
    const prev = selectedNodeRef.current;
    const prevData = (prev?.data as Record<string, unknown> | undefined) ?? undefined;
    const nextData = selectedNode.data as Record<string, unknown>;
    // Check if meaningful properties have changed (not just position)
    if (
      !prev ||
      prev.id !== selectedNode.id ||
      prev.type !== selectedNode.type ||
      prevData?.['kind'] !== nextData['kind'] ||
      prevData?.['label'] !== nextData['label'] ||
      prevData?.['sessionId'] !== nextData['sessionId'] ||
      !deepEqual(prevData?.['state'], nextData['state']) ||
      !deepEqual(prevData?.['params'], nextData['params'])
    ) {
      selectedNodeRef.current = selectedNode;
    }
    return selectedNodeRef.current;
  }, [selectedNode]);

  // Map definitions by kind for quick lookup (must be declared before use)
  const defByKind = React.useMemo(() => {
    const map = new Map<string, NodeDefinition>();
    for (const def of nodeDefinitions) map.set(def.kind, def);
    return map;
  }, [nodeDefinitions]);

  const selectedNodeDefinition = (() => {
    if (!selectedNode) return null;
    const kind = (selectedNode.data as { kind?: string }).kind;
    if (!kind) return null;
    return defByKind.get(kind) ?? null;
  })();

  // Get global WebSocket connection status
  const { isConnected: globalIsConnected } = useWebSocket();

  // Fetch session list
  const { data: sessions = [], isLoading: isLoadingSessions } = useSessionList();

  // Ensure every known session has a Zustand store entry so that
  // WS state events (which the server broadcasts for all visible
  // sessions) are persisted and drive the session-list status badges.
  useEffect(() => {
    const store = useSessionStore.getState();
    for (const s of sessions) {
      if (!store.getSession(s.id)) {
        store.initSession(s.id, true);
      }
    }
  }, [sessions]);

  // Memoize the selected session to prevent unnecessary re-renders
  // Uses a ref to store previous value and only updates when data actually changes (deep comparison)
  const prevSelectedSessionRef = React.useRef<
    { id: string; name: string | null; created_at: string } | undefined
  >(undefined);
  const selectedSession = React.useMemo(() => {
    const found = sessions.find((s) => s.id === selectedSessionId);
    const prev = prevSelectedSessionRef.current;

    // If both are undefined/null, return undefined
    if (!found && !prev) return undefined;

    // If one is undefined, update and return the new value
    if (!found || !prev) {
      prevSelectedSessionRef.current = found;
      return found;
    }

    // Deep comparison: if all fields match, return previous reference to prevent re-renders
    if (found.id === prev.id && found.name === prev.name && found.created_at === prev.created_at) {
      return prev;
    }

    // Data changed, update ref and return new value
    prevSelectedSessionRef.current = found;
    return found;
  }, [sessions, selectedSessionId]);

  // Auto-select the first session when none is selected (e.g., initial load)
  useEffect(() => {
    if (!selectedSessionId && !isLoadingSessions && sessions.length > 0) {
      const sessionId = sessions[0].id;
      setSelectedSessionId(sessionId);

      const savedPos = getNodePositions(sessionId);
      setNeedsAutoLayout(Object.keys(savedPos).length === 0);
      setNeedsFit(true);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- setNeedsAutoLayout/setNeedsFit are stable useState setters declared later
  }, [selectedSessionId, isLoadingSessions, sessions, getNodePositions]);

  // Pipeline data for the selected session is fetched once by useSession
  // (staleTime: Infinity) and kept current by live WebSocket events
  // (nodeadded, noderemoved, connectionadded, connectionremoved).
  // No periodic polling is needed — it would only introduce stale REST
  // data that overwrites live state and reverts local edits.

  // Subscribe to selected session.
  // nodeStates is intentionally NOT consumed from this hook — see the
  // useNodeStatesSubscription block below for the reasoning.
  const {
    pipeline,
    // nodeStats not used here - NodeStateIndicator fetches directly from session store
    isConnected: sessionIsConnected,
    tuneNode,
    tuneNodeConfig,
    addNode,
    removeNode,
    connectPins,
    disconnectPins,
  } = useSession(selectedSessionId);

  // Lightweight hook for dot-notation path updates: deep-merges locally
  // into the atom and sends only the partial to the server (unlike
  // useSession.tuneNodeConfig which shallow-merges and sends as-is).
  const { tuneNodeConfig: tuneNodeConfigDeep } = useTuneNode(selectedSessionId);

  // Use session-specific connection status if a session is selected, otherwise use global
  const isConnected = selectedSessionId ? sessionIsConnected : globalIsConnected;

  // Preview: watch-only MoQ connection from Monitor view.
  const {
    isPreviewConnected,
    isPreviewLoading,
    previewError,
    handleStartPreview,
    handleStopPreview,
  } = useMonitorPreview(selectedSessionId);

  // Use ref to avoid recreating callback when pipeline changes
  const pipelineRef = useRef(pipeline);
  pipelineRef.current = pipeline;

  // Topology signature: only changes when nodes/kinds or connections change
  const topoKey = React.useMemo(() => {
    if (!pipeline) return '';
    const names = Object.keys(pipeline.nodes).sort();
    const kinds = names.map((n) => `${n}:${pipeline.nodes[n].kind}`);
    const conns = pipeline.connections
      .map((c: Connection) => `${c.from_node}:${c.from_pin}>${c.to_node}:${c.to_pin}`)
      .sort();
    // Include runtime schema keys so topology rebuilds when schemas arrive
    // after the initial build (e.g. Slint property discovery).
    const runtimeKeys = Object.keys(pipeline.runtime_schemas ?? {}).sort();
    const key = JSON.stringify([kinds, conns, runtimeKeys]);
    viewsLogger.debug('topoKey recalculated:', key.substring(0, 100));
    return key;
  }, [pipeline]);

  // Auto-layout + fit-view hook
  const { setNeedsAutoLayout, setNeedsFit, handleAutoLayout } = useAutoLayout({
    pipeline,
    selectedSessionId,
    nodesLength: nodes.length,
    setNodes,
    rf,
    updateNodePosition,
  });

  // Throttled Zustand→ReactFlow patching bridge
  const { topoEffectRanRef } = useNodeStatesSubscription({
    selectedSessionId,
    setNodes,
    setEdges,
    pipelineRef,
    topoKey,
  });

  // When a session is destroyed, the optimistic removal empties the list
  // before React processes the batched setSelectedSessionId(null) from
  // handleConfirmQuickDelete.  Eagerly clear the selection here so the
  // badge and "Delete" control disappear immediately.
  //
  // The ref prevents this from fighting with the nav-state auto-select:
  // we only clear selection for sessions that were *previously seen* in
  // the list and then vanished (i.e., destroyed), not for a session ID
  // that was just set via navigation state and hasn't appeared yet.
  const sessionSeenInListRef = useRef(false);
  if (selectedSession) {
    sessionSeenInListRef.current = true;
  }
  useEffect(() => {
    if (
      selectedSessionId &&
      !selectedSession &&
      !isLoadingSessions &&
      sessionSeenInListRef.current
    ) {
      sessionSeenInListRef.current = false;
      setSelectedSessionId(null);
    }
  }, [selectedSessionId, selectedSession, isLoadingSessions]);

  // Helper to validate parameter value against schema
  const validateParamValue = useCallback(
    (nodeId: string, paramKey: string, value: unknown): string | null => {
      const node = pipeline?.nodes[nodeId];
      if (!node) return null;

      const nodeDef = nodeDefinitions.find((d) => d.kind === node.kind);
      if (!nodeDef) return null;

      const schema = nodeDef.param_schema as
        | {
            properties?: Record<
              string,
              { type?: string; minimum?: number; maximum?: number; multipleOf?: number }
            >;
          }
        | undefined;
      const propSchema = schema?.properties?.[paramKey];
      if (!propSchema) return null;

      return validateValue(value, propSchema);
    },
    [pipeline, nodeDefinitions]
  );

  // Ref indirection: keeps stableOnParamChange identity stable across
  // pipeline reference changes.  validateParamValue changes whenever the
  // pipeline object changes (e.g. param echo-back from server), but we
  // don't want that to cascade into new node data objects and break
  // React.memo on every node component.
  const validateParamValueRef = useRef(validateParamValue);
  validateParamValueRef.current = validateParamValue;

  // Memoized param change handler for right pane
  const handleRightPaneParamChange = useCallback(
    (nodeId: string, key: string, value: unknown) => {
      // Validate before sending to server
      const error = validateParamValueRef.current(nodeId, key, value);
      if (error) {
        toast.error(`Invalid value for ${key}: ${error}`);
        return;
      }

      // Dot-notation paths need nested payload (same deep-merge logic as
      // stableOnParamChange — see comment there for details).
      dispatchParamUpdate(nodeId, key, value, tuneNode, tuneNodeConfigDeep);
    },
    [toast, tuneNode, tuneNodeConfigDeep]
  );

  // Memoized label change handler (currently no-op)
  const handleRightPaneLabelChange = useCallback(() => {}, []);

  // Memoized handlers for TopControls to prevent re-renders
  const handleDeleteModalOpen = useCallback(() => {
    setShowDeleteModal(true);
  }, []);

  const onConnect = React.useCallback(
    (connection: RFConnection) => {
      return createOnConnect(
        nodesRefForCallbacks.current,
        setEdges,
        (conn: RFConnection) => {
          const from_pin = conn.sourceHandle || 'out';
          const to_pin = conn.targetHandle || 'in';
          connectPins(conn.source, from_pin, conn.target, to_pin);
        },
        edgesRefForCallbacks.current,
        setNodes
      )(connection);
    },
    [createOnConnect, setEdges, connectPins, setNodes]
  );

  const onEdgesDelete = (deleted: Edge[]) => {
    deleted.forEach((e) => {
      const from_pin = e.sourceHandle || 'out';
      const to_pin = e.targetHandle || 'in';
      disconnectPins(e.source, from_pin, e.target, to_pin);
    });
  };

  const onNodesDelete = (deleted: RFNode[]) => {
    deleted.forEach((n) => {
      removeNode(n.id);
    });
  };

  // Deletion is handled by React Flow's built-in delete key via onNodesDelete/onEdgesDelete.

  // Helpers to add nodes with sensible defaults
  const generateName = (kind: string) => {
    const existing = pipeline ? Object.keys(pipeline.nodes) : [];
    let i = 1;
    let candidate = `${kind}_${i}`;
    while (existing.includes(candidate)) {
      i += 1;
      candidate = `${kind}_${i}`;
    }
    return candidate;
  };

  const defaultParamsForKind = (kind: string): Record<string, unknown> => {
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
  };

  const onDragStart = useCallback(
    (event: React.DragEvent, nodeType: string) => {
      setType(nodeType);
      event.dataTransfer.setData('text/plain', nodeType);
      event.dataTransfer.effectAllowed = 'move';
    },
    [setType]
  );

  // Track previous topoKey to avoid unnecessary rebuilds
  const prevTopoKeyForTopologyRef = useRef<string>('');

  // Helper: Resolve node position from various sources (previous, pending, saved, or default)
  const resolveNodePosition = useCallback(
    (
      nodeName: string,
      prevPositions: Map<string, { x: number; y: number }>,
      savedPositions: Record<string, { x: number; y: number }>
    ): { position: { x: number; y: number }; fromPending: boolean } => {
      let pos = prevPositions.get(nodeName);
      let fromPending = false;

      // Check pending positions from node drops
      if (!pos && pendingNodePositions.current.has(nodeName)) {
        pos = pendingNodePositions.current.get(nodeName)!;
        pendingNodePositions.current.delete(nodeName);
        fromPending = true;
      }

      // Check saved positions from position store
      if (!pos && savedPositions[nodeName]) {
        pos = savedPositions[nodeName];
      }

      return {
        position: pos ?? { x: 0, y: 0 },
        fromPending,
      };
    },
    []
  );

  /**
   * Helper to reconstruct dynamic input pins from connections and previous state.
   * Reduces nesting by extracting pin reconstruction logic.
   */
  const reconstructDynamicInputs = useCallback(
    (
      nodeName: string,
      dynamicTemplate: InputPin,
      activePipeline: Pipeline,
      prevNode: RFNode | undefined
    ): InputPin[] => {
      const dynamicPins = new Map<string, InputPin>();

      // Add pins from active connections
      const incomingConnections = activePipeline.connections.filter(
        (conn) => conn.to_node === nodeName
      );
      for (const conn of incomingConnections) {
        if (/^in_\d+$/.test(conn.to_pin)) {
          dynamicPins.set(conn.to_pin, {
            name: conn.to_pin,
            accepts_types: dynamicTemplate.accepts_types,
            cardinality: 'One',
          });
        }
      }

      // Preserve disconnected dynamic pins from previous state
      const prevInputs = prevNode?.data.inputs as InputPin[] | undefined;
      if (prevInputs) {
        for (const pin of prevInputs) {
          if (pin.cardinality === 'One' && /^in_\d+$/.test(pin.name)) {
            if (!dynamicPins.has(pin.name)) {
              dynamicPins.set(pin.name, pin);
            }
          }
        }
      }

      return Array.from(dynamicPins.values());
    },
    []
  );

  /**
   * Helper to reconstruct dynamic output pins from connections and previous state.
   * Reduces nesting by extracting pin reconstruction logic.
   */
  const reconstructDynamicOutputs = useCallback(
    (
      nodeName: string,
      dynamicTemplate: OutputPin,
      activePipeline: Pipeline,
      prevNode: RFNode | undefined
    ): OutputPin[] => {
      const dynamicPins = new Map<string, OutputPin>();

      // Add pins from active connections
      const outgoingConnections = activePipeline.connections.filter(
        (conn) => conn.from_node === nodeName
      );
      for (const conn of outgoingConnections) {
        if (/^out_\d+$/.test(conn.from_pin)) {
          dynamicPins.set(conn.from_pin, {
            name: conn.from_pin,
            produces_type: dynamicTemplate.produces_type,
            cardinality: 'One',
          });
        }
      }

      // Preserve disconnected dynamic pins from previous state
      const prevOutputs = prevNode?.data.outputs as OutputPin[] | undefined;
      if (prevOutputs) {
        for (const pin of prevOutputs) {
          if (/^out_\d+$/.test(pin.name)) {
            if (!dynamicPins.has(pin.name)) {
              dynamicPins.set(pin.name, pin);
            }
          }
        }
      }

      return Array.from(dynamicPins.values());
    },
    []
  );

  // Helper: Resolve dynamic pins for nodes that support them
  const resolveDynamicPins = useCallback(
    (
      nodeDefinition: NodeDefinition | undefined,
      nodeName: string,
      activePipeline: Pipeline,
      baseInputs: InputPin[],
      baseOutputs: OutputPin[]
    ): { finalInputs: InputPin[]; finalOutputs: OutputPin[] } => {
      const hasDynamicInputs =
        nodeDefinition?.inputs.some(
          (pin) => typeof pin.cardinality === 'object' && 'Dynamic' in pin.cardinality
        ) ?? false;
      const hasDynamicOutputs =
        nodeDefinition?.outputs.some(
          (pin) => typeof pin.cardinality === 'object' && 'Dynamic' in pin.cardinality
        ) ?? false;

      let finalInputs = baseInputs;
      let finalOutputs = baseOutputs;

      // Reconstruct dynamic input pins from connections
      if (hasDynamicInputs) {
        const dynamicTemplate = nodeDefinition?.inputs.find(
          (pin) => typeof pin.cardinality === 'object' && 'Dynamic' in pin.cardinality
        );

        if (dynamicTemplate) {
          const prevNode = nodes.find((n) => n.id === nodeName);
          const dynamicInputs = reconstructDynamicInputs(
            nodeName,
            dynamicTemplate,
            activePipeline,
            prevNode
          );
          finalInputs = [...baseInputs, ...dynamicInputs];
        }
      }

      // Reconstruct dynamic output pins from connections
      if (hasDynamicOutputs) {
        const dynamicTemplate = nodeDefinition?.outputs.find(
          (pin) => typeof pin.cardinality === 'object' && 'Dynamic' in pin.cardinality
        );

        if (dynamicTemplate) {
          const prevNode = nodes.find((n) => n.id === nodeName);
          const dynamicOutputs = reconstructDynamicOutputs(
            nodeName,
            dynamicTemplate,
            activePipeline,
            prevNode
          );
          finalOutputs = [...baseOutputs, ...dynamicOutputs];
        }
      }

      return { finalInputs, finalOutputs };
    },
    [nodes, reconstructDynamicInputs, reconstructDynamicOutputs]
  );

  // Update nodes and edges when pipeline topology changes (nodes added/removed/reconnected)
  // Other state updates (nodeStates, nodeStats, params) are handled by separate lightweight effects
  /**
   * This effect has 38 statements and complexity of 21, which exceeds limits.
   * The complexity is inherent to the task of building a React Flow graph from a pipeline:
   * - Early returns for optimization (topoKey check, no pipeline case)
   * - Topological sorting to get node order
   * - Iterating through nodes to build Node objects with position, state, pins
   * - Building edges with validation
   * - Generating YAML representation
   * Helper functions have been extracted where possible, but further breakdown
   * would fragment the graph-building logic across multiple locations.
   */
  // eslint-disable-next-line max-statements -- Core graph-building logic
  useEffect(() => {
    viewsLogger.debug(
      'Topology effect check, prev:',
      prevTopoKeyForTopologyRef.current.substring(0, 30),
      'curr:',
      topoKey.substring(0, 30),
      'match:',
      prevTopoKeyForTopologyRef.current === topoKey
    );

    // Skip if topoKey hasn't actually changed
    if (prevTopoKeyForTopologyRef.current === topoKey && nodes.length > 0) {
      viewsLogger.debug('Skipping topology effect, topoKey unchanged');
      return;
    }
    prevTopoKeyForTopologyRef.current = topoKey;

    if (!pipeline) {
      viewsLogger.debug('Topology effect: No pipeline, clearing nodes');
      setNodes([]);
      setEdges([]);
      setYamlString('');
      return;
    }

    viewsLogger.debug('Topology effect triggered, topoKey:', topoKey.substring(0, 50) + '...');

    // Preserve existing node positions; do not auto-layout during edits.
    const { levels, sortedLevels } = topoLevelsFromPipeline(pipeline);
    const orderedNames = orderedNamesFromLevels(levels, sortedLevels);

    const prevPositions = new Map(nodes.map((n) => [n.id, n.position]));

    // Get saved positions from position store
    const savedPositions = selectedSessionId ? getNodePositions(selectedSessionId) : {};

    const newNodes: RFNode[] = [];
    for (const nodeName of orderedNames) {
      const apiNode = pipeline.nodes[nodeName];
      if (!apiNode) continue;

      // Resolve node position from various sources
      const { position: pos, fromPending: positionFromPending } = resolveNodePosition(
        nodeName,
        prevPositions,
        savedPositions
      );

      // Save position to position store if it came from pending (newly dropped)
      if (positionFromPending && selectedSessionId) {
        updateNodePosition(selectedSessionId, nodeName, pos);
      }

      // Use real-time state from Jotai atom if available, otherwise use pipeline state.
      // Read directly from the default store (non-reactive) since the topology effect
      // only runs on structural changes, not on every node-state transition.
      const nodeState =
        (selectedSessionId
          ? defaultSessionStore.get(nodeStateAtom(nodeKey(selectedSessionId, nodeName)))
          : null) ?? apiNode.state;

      // Get base pins from definition and resolve dynamic pins
      const baseInputs = defByKind.get(apiNode.kind)?.inputs ?? [];
      const baseOutputs = defByKind.get(apiNode.kind)?.outputs ?? [];
      const nodeDefinition = defByKind.get(apiNode.kind);

      const { finalInputs, finalOutputs } = resolveDynamicPins(
        nodeDefinition,
        nodeName,
        pipeline,
        baseInputs,
        baseOutputs
      );

      const nodeDef = defByKind.get(apiNode.kind);

      // Merge runtime param schema (if any) with the static per-kind schema.
      // Runtime schemas are per-instance overrides discovered after node init
      // (e.g. Slint component properties enumerated from the compiled .slint).
      const runtimeSchema = pipeline.runtime_schemas?.[nodeName] as JsonSchema | undefined;
      const effectiveNodeDef =
        runtimeSchema && nodeDef
          ? {
              ...nodeDef,
              param_schema: deepMergeSchemas(
                nodeDef.param_schema as JsonSchema | undefined,
                runtimeSchema
              ),
            }
          : nodeDef;

      // Build node object using helper function
      const node = buildNodeObject({
        nodeName,
        apiNode,
        position: pos,
        nodeState,
        finalInputs,
        finalOutputs,
        nodeDef: effectiveNodeDef,
        stableOnParamChange,
        stableOnConfigChange,
        selectedSessionId,
      });

      newNodes.push(node);
    }

    // Build edges using helper function
    const newEdges = buildEdgesFromConnections(pipeline.connections, newNodes);

    viewsLogger.debug('Setting', newNodes.length, 'nodes and', newEdges.length, 'edges');
    // Batch node and edge updates to prevent double render
    React.startTransition(() => {
      setNodes(newNodes);
      setEdges(newEdges);
      topoEffectRanRef.current = true;
    });

    // Generate YAML using helper function
    const yamlString = generatePipelineYaml(pipeline, orderedNames);
    setYamlString(yamlString);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [topoKey, defByKind, selectedSessionId, tuneNode]);

  // Stable callback for param changes - always sends directly to server
  // Uses ref indirection for validateParamValue to keep identity stable
  // across pipeline reference changes (see validateParamValueRef above).
  const stableOnParamChange = useCallback(
    (nodeId: string, paramName: string, value: unknown) => {
      // Validate before sending to server
      const error = validateParamValueRef.current(nodeId, paramName, value);
      if (error) {
        toast.error(`Invalid value for ${paramName}: ${error}`);
        return;
      }

      // Dot-notation paths (e.g. "properties.show") need buildParamUpdate to
      // produce the correct nested UpdateParams payload.  tuneNodeConfigDeep
      // deep-merges locally into the atom (preserving sibling nested
      // properties) and sends only the partial to the server.
      dispatchParamUpdate(nodeId, paramName, value, tuneNode, tuneNodeConfigDeep);
    },
    [toast, tuneNode, tuneNodeConfigDeep]
  );

  // Stable callback for full-config updates (compositor nodes).
  const stableOnConfigChange = useCallback(
    (nodeId: string, config: Record<string, unknown>) => {
      tuneNodeConfig(nodeId, config);
    },
    [tuneNodeConfig]
  );

  // NOTE: fitView is triggered only by:
  // 1. Auto-layout effect (when needsAutoLayout is true)
  // 2. needsFit effect (when needsFit is true)
  // Avoid auto-fitting on every node change to prevent disruption during editing.

  // Keep YAML up to date with live (Zustand) param overrides
  // Only runs when params change, not when nodes move
  useEffect(() => {
    if (!pipeline) {
      setYamlString('');
      return;
    }

    const yamlObject: { nodes: Record<string, unknown> } = { nodes: {} };

    // Use topological order to keep YAML stable (not affected by canvas positions)
    const { levels, sortedLevels } = topoLevelsFromPipeline(pipeline);
    const sortedNames = orderedNamesFromLevels(levels, sortedLevels);

    for (const nodeName of sortedNames) {
      const apiNode = pipeline.nodes[nodeName];
      if (!apiNode) continue;

      const needs = pipeline.connections
        .filter((c: Connection) => c.to_node === nodeName)
        .map((c: Connection) => c.from_node);

      const nodeConfig: Record<string, unknown> = { kind: apiNode.kind };

      const paramKey = selectedSessionId ? `${selectedSessionId}\0${nodeName}` : nodeName;
      const overrides = defaultSessionStore.get(nodeParamsAtom(paramKey));
      const rawParams = { ...(apiNode.params || {}), ...(overrides || {}) };
      // Strip transient sync metadata (_sender, _rev, etc.) from YAML export.
      const mergedParams: Record<string, unknown> = {};
      for (const [key, value] of Object.entries(rawParams)) {
        if (!key.startsWith('_')) {
          mergedParams[key] = value;
        }
      }
      if (Object.keys(mergedParams).length > 0) {
        nodeConfig['params'] = mergedParams;
      }

      if (needs.length === 1) {
        nodeConfig['needs'] = needs[0];
      } else if (needs.length > 1) {
        nodeConfig['needs'] = needs;
      }

      yamlObject.nodes[nodeName] = nodeConfig;
    }

    setYamlString(dump(yamlObject, { skipInvalid: true }));
  }, [pipeline, selectedSessionId]);

  const onConnectEnd: OnConnectEnd = React.useCallback(
    (event, connectionState) => {
      return createOnConnectEnd(nodesRefForCallbacks.current, edgesRefForCallbacks.current)(
        event,
        connectionState
      );
    },
    [createOnConnectEnd]
  );

  const handleDuplicateNode = (nodeId: string) => {
    // In monitor mode, we could potentially duplicate via WebSocket
    viewsLogger.debug('Duplicate node:', nodeId);
  };

  const handleDeleteNode = (nodeId: string) => {
    removeNode(nodeId);
  };

  const onDragOver = (event: React.DragEvent) => {
    event.preventDefault();
    event.dataTransfer.dropEffect = 'move';
  };

  const onDrop = (event: React.DragEvent) => {
    event.preventDefault();
    if (!type) {
      return;
    }

    // Calculate drop position in flow coordinates
    const position = screenToFlow({
      x: event.clientX,
      y: event.clientY,
    });

    const kind = type;
    const nodeId = generateName(kind);
    const params = defaultParamsForKind(kind);

    // Cache the position for when the node appears in the pipeline
    pendingNodePositions.current.set(nodeId, position);

    // Send to server immediately
    addNode(nodeId, kind, params);

    setType(null);
  };

  const handleSessionClick = useCallback(
    (sessionId: string) => {
      // Use startTransition to make session loading non-blocking
      // This allows the UI to stay responsive while loading heavy pipelines
      React.startTransition(() => {
        setSelectedSessionId(sessionId);
        // Check if this session has saved positions
        const savedPos = getNodePositions(sessionId);
        const hasPositions = Object.keys(savedPos).length > 0;

        viewsLogger.debug('Session click, hasPositions:', hasPositions);
        // Only auto-layout if no positions are saved
        setNeedsAutoLayout(!hasPositions);
        setNeedsFit(true);
      });
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps -- setNeedsAutoLayout/setNeedsFit are stable useState setters
    [getNodePositions]
  );

  const handleQuickDeleteSession = useCallback((sessionId: string) => {
    // Store which session to delete and show confirmation modal
    setSessionToDelete(sessionId);
  }, []);

  const handleConfirmQuickDelete = useCallback(async () => {
    if (!sessionToDelete) return;

    setIsDeletingSession(true);

    try {
      // Only tear down the preview when deleting the session that is
      // actually being previewed.  sessionToDelete can be any session
      // from the sidebar; stopping the preview unconditionally would
      // kill an unrelated active stream.
      if (sessionToDelete === selectedSessionId) {
        await handleStopPreview();
      }

      const wsService = getWebSocketService();

      const response = await wsService.send({
        type: 'request' as MessageType,
        correlation_id: uuidv4(),
        payload: {
          action: 'destroysession' as const,
          session_id: sessionToDelete,
        },
      });

      if (response.payload.action === 'sessiondestroyed') {
        toast.success(`Session deleted successfully`);
        clearSessionPositions(sessionToDelete);
        // If the deleted session was selected, clear selection
        if (selectedSessionId === sessionToDelete) {
          setSelectedSessionId(null);
        }
        setSessionToDelete(null);
      } else if (response.payload.action === 'error') {
        throw new Error(response.payload.message);
      }
    } catch (error) {
      viewsLogger.error('Failed to delete session:', error);
      toast.error(error instanceof Error ? error.message : 'Failed to delete session');
    } finally {
      setIsDeletingSession(false);
    }
  }, [sessionToDelete, selectedSessionId, toast, clearSessionPositions, handleStopPreview]);

  const handleDeleteSession = useCallback(async () => {
    if (!selectedSessionId) return;

    setIsDeletingSession(true);

    try {
      // Tear down preview/MoQ connection BEFORE destroying the session
      // to avoid SIGSEGV from WebCodecs operating on a dead stream.
      await handleStopPreview();

      const wsService = getWebSocketService();

      const response = await wsService.send({
        type: 'request' as MessageType,
        correlation_id: uuidv4(),
        payload: {
          action: 'destroysession' as const,
          session_id: selectedSessionId,
        },
      });

      if (response.payload.action === 'sessiondestroyed') {
        toast.success(`Session ${selectedSessionId} deleted successfully`);
        clearSessionPositions(selectedSessionId);
        setSelectedSessionId(null);
        setShowDeleteModal(false);
      } else if (response.payload.action === 'error') {
        throw new Error(response.payload.message);
      }
    } catch (error) {
      viewsLogger.error('Failed to delete session:', error);
      toast.error(error instanceof Error ? error.message : 'Failed to delete session');
    } finally {
      setIsDeletingSession(false);
    }
  }, [selectedSessionId, toast, clearSessionPositions, handleStopPreview]);

  const handleCancelDeleteModal = useCallback(() => setShowDeleteModal(false), []);
  const handleCancelQuickDelete = useCallback(() => setSessionToDelete(null), []);

  // Memoize left panel to prevent ResizableLayout from re-rendering
  const leftPanel = React.useMemo(
    () => (
      <LeftPanel
        isLoadingSessions={isLoadingSessions}
        sessions={sessions}
        selectedSessionId={selectedSessionId}
        onSessionClick={handleSessionClick}
        onSessionDelete={handleQuickDeleteSession}
        nodeDefinitions={nodeDefinitions}
        onDragStart={onDragStart}
        pluginKinds={pluginKinds}
        pluginTypes={pluginTypes}
      />
    ),
    [
      isLoadingSessions,
      sessions,
      selectedSessionId,
      handleSessionClick,
      handleQuickDeleteSession,
      nodeDefinitions,
      onDragStart,
      pluginKinds,
      pluginTypes,
    ]
  );

  // Memoize center panel to prevent ResizableLayout from re-rendering

  // - Only track nodes.length, not full nodes array (FlowCanvas handles position updates internally)
  // - selectedSession used instead of sessions array to prevent unnecessary re-renders
  const hasPipeline = !!pipeline;
  const centerPanel = React.useMemo(
    () => (
      <CenterPanelContainer>
        <CanvasTopBar>
          <TopLeftControls>
            <MonitorViewTitle />
            {selectedSession && <SessionInfoChip session={selectedSession} />}
          </TopLeftControls>
          <TopControls
            isConnected={isConnected}
            selectedSessionId={selectedSessionId}
            onDelete={handleDeleteModalOpen}
            onStartPreview={handleStartPreview}
            onStopPreview={handleStopPreview}
            isPreviewConnected={isPreviewConnected}
            isPreviewLoading={isPreviewLoading}
            previewError={previewError}
          />
        </CanvasTopBar>
        {selectedSessionId && nodes.length > 0 ? (
          <>
            <FlowCanvas
              nodes={nodes}
              edges={edges}
              nodeTypes={nodeTypes}
              onNodesChange={onNodesChangeBatched}
              onEdgesChange={onEdgesChange}
              colorMode={colorMode}
              onInit={onInit}
              defaultEdgeOptions={defaultEdgeOptions}
              editMode={true}
              onNodeDragStop={onNodeDragStop}
              onNodeDoubleClick={handleNodeDoubleClick}
              isValidConnection={
                isValidConnection
                  ? (conn) =>
                      isValidConnection(
                        conn,
                        nodesRefForCallbacks.current,
                        edgesRefForCallbacks.current
                      )
                  : undefined
              }
              onConnect={onConnect}
              onConnectEnd={onConnectEnd}
              onEdgesDelete={onEdgesDelete}
              onNodesDelete={onNodesDelete}
              onPaneClick={onPaneClick}
              onPaneContextMenu={onPaneContextMenu}
              onNodeContextMenu={onNodeContextMenu}
              onDrop={onDrop}
              onDragOver={onDragOver}
              reactFlowWrapper={reactFlowWrapper}
            />
            <Legend />
          </>
        ) : (
          <EmptyMonitorState>
            {selectedSessionId && !pipeline ? (
              <p>Loading pipeline...</p>
            ) : (
              <p>Select a session from the left panel to inspect its pipeline.</p>
            )}
          </EmptyMonitorState>
        )}
        {(isPreviewConnected || isPreviewLoading) && (
          <OutputPreviewPanel hasSession={selectedSessionId != null} conditionalRender />
        )}
      </CenterPanelContainer>
    ),
    // Intentional sparse dependencies for performance optimization:
    // - Only track nodes.length, not full nodes array (FlowCanvas handles position updates internally)
    // - selectedSession used instead of sessions array to prevent unnecessary re-renders
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [
      selectedSessionId,
      selectedSession,
      isConnected,
      nodes.length,
      hasPipeline,
      colorMode,
      onInit,
      handleStartPreview,
      handleStopPreview,
      isPreviewConnected,
      isPreviewLoading,
      previewError,
    ]
  );

  // Extract selected node label for YAML highlighting
  const selectedNodeLabel = React.useMemo(() => {
    return stableSelectedNode?.data?.label as string | undefined;
  }, [stableSelectedNode]);

  // Memoize right panel to prevent ResizableLayout from re-rendering
  const rightPanel = React.useMemo(
    () =>
      selectedSessionId && pipeline ? (
        <PipelineRightPane
          selectedNode={
            stableSelectedNode as RFNode<{
              label: string;
              kind: string;
              params: Record<string, unknown>;
            }> | null
          }
          selectedNodeDefinition={selectedNodeDefinition}
          selectedNodeLabel={selectedNodeLabel}
          rightPaneView={rightPaneView}
          setRightPaneView={setRightPaneView}
          yamlString={yamlString}
          onYamlChange={undefined}
          onParamChange={handleRightPaneParamChange}
          onLabelChange={handleRightPaneLabelChange}
          nodeDefinitions={nodeDefinitions}
          readOnly={false}
          yamlReadOnly={true}
          isMonitorView={true}
          sessionId={selectedSessionId}
        />
      ) : undefined,
    [
      selectedSessionId,
      pipeline,
      stableSelectedNode,
      selectedNodeDefinition,
      selectedNodeLabel,
      rightPaneView,
      setRightPaneView,
      yamlString,
      handleRightPaneParamChange,
      handleRightPaneLabelChange,
      nodeDefinitions,
    ]
  );

  return (
    <div style={{ height: '100%' }} data-testid="monitor-view">
      <ResizableLayout
        left={leftPanel}
        center={centerPanel}
        right={rightPanel}
        leftLabel="Sessions"
        centerLabel="Pipeline"
        rightLabel="Inspector"
      />
      {menu && (
        <ContextMenu
          onClick={onPaneClick}
          onDuplicate={handleDuplicateNode}
          onDelete={handleDeleteNode}
          {...menu}
        />
      )}
      {paneMenu && (
        <PaneContextMenu onClick={onPaneClick} onAutoLayout={handleAutoLayout} {...paneMenu} />
      )}
      <ConfirmModal
        isOpen={showDeleteModal}
        title="Delete Session"
        message={`Are you sure you want to delete session "${selectedSessionId}"? This action cannot be undone.`}
        confirmLabel="Delete"
        cancelLabel="Cancel"
        onConfirm={handleDeleteSession}
        onCancel={handleCancelDeleteModal}
        isLoading={isDeletingSession}
      />
      <ConfirmModal
        isOpen={sessionToDelete !== null}
        title="Delete Session"
        message={`Are you sure you want to delete session "${sessionToDelete}"? This will stop the pipeline and all running nodes. This action cannot be undone.`}
        confirmLabel="Delete"
        cancelLabel="Cancel"
        onConfirm={handleConfirmQuickDelete}
        onCancel={handleCancelQuickDelete}
        isLoading={isDeletingSession}
      />
    </div>
  );
};

const MonitorView: React.FC = () => {
  return (
    <ReactFlowProvider>
      <DnDProvider>
        <MonitorViewContent />
      </DnDProvider>
    </ReactFlowProvider>
  );
};

export default MonitorView;
