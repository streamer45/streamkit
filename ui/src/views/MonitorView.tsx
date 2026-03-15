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
import React, { useState, useEffect, useCallback, useRef } from 'react';
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
import {
  useMonitorSessionManager,
  type OnSessionActivated,
} from '@/hooks/useMonitorSessionManager';
import { useMonitorStaging } from '@/hooks/useMonitorStaging';
import { useMonitorYaml } from '@/hooks/useMonitorYaml';
import { useNodeStatesSubscription } from '@/hooks/useNodeStatesSubscription';
import { useReactFlowCommon } from '@/hooks/useReactFlowCommon';
import { useResolvedColorMode } from '@/hooks/useResolvedColorMode';
import { useSession } from '@/hooks/useSession';
import { useSessionsPrefetch } from '@/hooks/useSessionsPrefetch';
import { useWebSocket } from '@/hooks/useWebSocket';
import { useLayoutStore } from '@/stores/layoutStore';
import { usePluginStore } from '@/stores/pluginStore';
import { useSchemaStore } from '@/stores/schemaStore';
import { useSessionStore } from '@/stores/sessionStore';
import { useStagingStore } from '@/stores/stagingStore';
import type { NodeDefinition, Connection, Pipeline, InputPin, OutputPin } from '@/types/types';
import { topoLevelsFromPipeline, orderedNamesFromLevels } from '@/utils/dag';
import { deepEqual } from '@/utils/deepEqual';
import { validateValue } from '@/utils/jsonSchema';
import { viewsLogger } from '@/utils/logger';
import {
  buildEdgesFromConnections,
  buildNodeObject,
  generatePipelineYaml,
} from '@/utils/pipelineGraph';
import { validatePipeline } from '@/utils/pipelineValidation';
import { nodeTypes, defaultEdgeOptions } from '@/utils/reactFlowDefaults';

// Memoized view title to prevent re-renders during drag
const MonitorViewTitle = React.memo(() => <ViewTitle>Monitor</ViewTitle>);

/**
 * Main content component for the Monitor view.
 *
 * Heavy concerns are delegated to extracted custom hooks:
 * - useMonitorSessionManager – session selection, auto-select, deletion
 * - useMonitorStaging – staging lifecycle (enter / discard / commit)
 * - useMonitorYaml – YAML regeneration and YAML-edit handler
 *
 * This component wires them together and owns the rendering, topology
 * computation, and ReactFlow integration.
 */
// eslint-disable-next-line max-statements -- Main view component with many hooks and state management
const MonitorViewContent: React.FC = () => {
  const [nodes, setNodes, onNodesChangeInternal] = useNodesState<RFNode>([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>([]);

  // ── Low-priority dimension changes ────────────────────────────────────
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

  const nodeDefinitions = useSchemaStore(useShallow((s) => s.nodeDefinitions));
  const plugins = usePluginStore(useShallow((s) => s.plugins));
  const pluginKinds = React.useMemo(() => new Set(plugins.map((p) => p.kind)), [plugins]);
  const pluginTypes = React.useMemo(
    () => new Map(plugins.map((p) => [p.kind, p.plugin_type])),
    [plugins]
  );

  const [selectedNodes, setSelectedNodes] = useState<string[]>([]);
  const [rightPaneView, setRightPaneView] = useState<'yaml' | 'inspector' | 'telemetry'>('yaml');
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

  // ── Session management (extracted hook) ───────────────────────────────
  // Bridge ref: useMonitorSessionManager notifies us when a session is
  // activated so we can trigger auto-layout.  The ref avoids a circular
  // dependency between session selection and useAutoLayout.
  const onSessionActivatedRef = useRef<OnSessionActivated>(() => {});

  const {
    selectedSessionId,
    selectedSession,
    sessions,
    isLoadingSessions,
    showDeleteModal,
    setShowDeleteModal,
    sessionToDelete,
    setSessionToDelete,
    isDeletingSession,
    handleSessionClick,
    handleQuickDeleteSession,
    handleConfirmQuickDelete,
    handleDeleteSession,
    handleDeleteModalOpen,
  } = useMonitorSessionManager({ onSessionActivatedRef });

  // Get global WebSocket connection status
  const { isConnected: globalIsConnected } = useWebSocket();

  // Prefetch pipeline data for all sessions to enable status display
  useSessionsPrefetch(sessions);

  // Subscribe to selected session.
  const {
    pipeline,
    isConnected: sessionIsConnected,
    isLoading: isLoadingPipeline,
    tuneNode,
    tuneNodeConfig,
    addNode,
    removeNode,
    connectPins,
    disconnectPins,
    applyBatch,
  } = useSession(selectedSessionId);

  const isConnected = selectedSessionId ? sessionIsConnected : globalIsConnected;

  // Preview: watch-only MoQ connection from Monitor view.
  const { isPreviewConnected, handleStartPreview } = useMonitorPreview(selectedSessionId, pipeline);

  // Use ref to avoid recreating callback when pipeline changes
  const pipelineRef = useRef<Pipeline | null>(pipeline ?? null);
  pipelineRef.current = pipeline ?? null;

  // Keep refs to avoid recreating callbacks on every drag
  const nodesRefForCallbacks = React.useRef(nodes);
  const edgesRefForCallbacks = React.useRef(edges);
  React.useEffect(() => {
    nodesRefForCallbacks.current = nodes;
    edgesRefForCallbacks.current = edges;
  }, [nodes, edges]);

  // Keep a ref to the latest nodes for callbacks that need them
  const nodesRef = React.useRef(nodes);
  React.useEffect(() => {
    nodesRef.current = nodes;
  }, [nodes]);

  // ── Staging mode (extracted hook) ─────────────────────────────────────
  const {
    stagingData,
    isInStagingMode,
    stagedPipeline,
    addStagedNode,
    removeStagedNode,
    addStagedConnection,
    removeStagedConnection,
    updateStagedNodeParams,
    updateNodePosition,
    setValidationErrors,
    handleEnterStagingMode,
    handleDiscardChanges,
    handleCommitChanges,
    onNodeDragStop,
  } = useMonitorStaging({
    selectedSessionId,
    pipelineRef,
    nodesRef,
    applyBatch,
  });

  const editMode = isInStagingMode;

  // getNodePositions for the topology effect (not exposed from useMonitorStaging
  // because it's also used by useMonitorSessionManager internally)
  const getNodePositions = useStagingStore((s) => s.getNodePositions);

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
      prevData?.['isStaged'] !== nextData['isStaged'] ||
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

  // Run validation whenever staged pipeline changes
  useEffect(() => {
    if (!selectedSessionId || !isInStagingMode || !stagedPipeline) return;

    viewsLogger.debug('Running validation');
    const errors = validatePipeline(stagedPipeline, defByKind);
    setValidationErrors(selectedSessionId, errors);

    // Show toast for validation errors
    const errorCount = errors.filter((e) => e.type === 'error').length;
    if (errorCount > 0) {
      toast.error(
        `Validation failed: ${errorCount} error${errorCount > 1 ? 's' : ''} found. Fix them before committing.`
      );
    }
  }, [selectedSessionId, isInStagingMode, stagedPipeline, defByKind, setValidationErrors, toast]);

  // Topology signature: only changes when nodes/kinds or connections change
  const topoKey = React.useMemo(() => {
    const activePipeline = isInStagingMode && stagedPipeline ? stagedPipeline : pipeline;
    if (!activePipeline) return '';
    const names = Object.keys(activePipeline.nodes).sort();
    const kinds = names.map((n) => `${n}:${activePipeline.nodes[n].kind}`);
    const conns = activePipeline.connections
      .map((c: Connection) => `${c.from_node}:${c.from_pin}>${c.to_node}:${c.to_pin}`)
      .sort();
    const key = JSON.stringify([kinds, conns]);
    viewsLogger.debug(
      'topoKey recalculated:',
      key.substring(0, 100),
      'isInStagingMode:',
      isInStagingMode
    );
    return key;
  }, [stagedPipeline, pipeline, isInStagingMode]);

  // Auto-layout + fit-view hook
  const { setNeedsAutoLayout, setNeedsFit, handleAutoLayout } = useAutoLayout({
    pipeline,
    selectedSessionId,
    nodesLength: nodes.length,
    setNodes,
    rf,
    updateNodePosition,
  });

  // Wire the session-activation bridge now that setNeedsAutoLayout is available
  onSessionActivatedRef.current = (_sessionId: string, hasPositions: boolean) => {
    setNeedsAutoLayout(!hasPositions);
    setNeedsFit(true);
  };

  // Throttled Zustand→ReactFlow patching bridge
  const { topoEffectRanRef } = useNodeStatesSubscription({
    selectedSessionId,
    setNodes,
    setEdges,
    pipelineRef,
    topoKey,
  });

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

  // Memoized param change handler for right pane
  const handleRightPaneParamChange = useCallback(
    (nodeId: string, key: string, value: unknown) => {
      const isStaged = isInStagingMode && stagingData?.stagedNodes.has(nodeId);
      if (isStaged && selectedSessionId) {
        updateStagedNodeParams(selectedSessionId, nodeId, { [key]: value });
        return;
      }

      // Validate before sending to server
      const error = validateParamValue(nodeId, key, value);
      if (error) {
        toast.error(`Invalid value for ${key}: ${error}`);
        return;
      }

      tuneNode(nodeId, key, value);
    },
    [
      isInStagingMode,
      stagingData?.stagedNodes,
      selectedSessionId,
      updateStagedNodeParams,
      validateParamValue,
      toast,
      tuneNode,
    ]
  );

  // Memoized label change handler (currently no-op)
  const handleRightPaneLabelChange = useCallback(() => {}, []);

  // ── YAML handling (extracted hook) ────────────────────────────────────
  const { yamlString, setYamlFromTopology, handleYamlChange } = useMonitorYaml({
    selectedSessionId,
    pipeline,
    isInStagingMode,
    stagedPipeline,
    stagingData,
    nodeDefinitions,
    tuneNode,
  });

  const onConnect = React.useCallback(
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

  const onEdgesDelete = (deleted: Edge[]) => {
    deleted.forEach((e) => {
      const from_pin = e.sourceHandle || 'out';
      const to_pin = e.targetHandle || 'in';

      if (isInStagingMode && selectedSessionId) {
        // Remove from staging store
        removeStagedConnection(selectedSessionId, {
          from_node: e.source,
          from_pin,
          to_node: e.target,
          to_pin,
        });
      } else {
        // Send to server immediately (monitor mode)
        disconnectPins(e.source, from_pin, e.target, to_pin);
      }
    });
  };

  const onNodesDelete = (deleted: RFNode[]) => {
    deleted.forEach((n) => {
      if (isInStagingMode && selectedSessionId) {
        // Remove from staging store
        removeStagedNode(selectedSessionId, n.id);
      } else {
        // Send to server immediately (monitor mode)
        removeNode(n.id);
      }
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

      // Check saved positions from staging store
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
          // P1 1D: use nodesRef instead of `nodes` to avoid unstable deps
          const prevNode = nodesRef.current.find((n) => n.id === nodeName);
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
          const prevNode = nodesRef.current.find((n) => n.id === nodeName);
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
    [reconstructDynamicInputs, reconstructDynamicOutputs]
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

    // Skip if topoKey hasn't actually changed (entering/exiting staging with same topology)
    if (prevTopoKeyForTopologyRef.current === topoKey && nodes.length > 0) {
      viewsLogger.debug('Skipping topology effect, topoKey unchanged');
      return;
    }
    prevTopoKeyForTopologyRef.current = topoKey;

    // Use staged pipeline when in staging mode, otherwise use live pipeline
    const activePipeline =
      isInStagingMode && stagingData?.stagedPipeline ? stagingData.stagedPipeline : pipeline;

    if (!activePipeline) {
      viewsLogger.debug('Topology effect: No pipeline, clearing nodes');
      setNodes([]);
      setEdges([]);
      setYamlFromTopology('');
      return;
    }

    viewsLogger.debug('Topology effect triggered, topoKey:', topoKey.substring(0, 50) + '...');

    // Preserve existing node positions; do not auto-layout during edits.
    const { levels, sortedLevels } = topoLevelsFromPipeline(activePipeline);
    const orderedNames = orderedNamesFromLevels(levels, sortedLevels);

    const prevPositions = new Map(nodes.map((n) => [n.id, n.position]));

    // Get saved positions from staging store if in staging mode
    const savedPositions =
      isInStagingMode && selectedSessionId ? getNodePositions(selectedSessionId) : {};

    // P1 1E: hoist getState() above the loop — one call instead of N
    const currentNodeStates = selectedSessionId
      ? (useSessionStore.getState().getSession(selectedSessionId)?.nodeStates ?? {})
      : {};

    const newNodes: RFNode[] = [];
    for (const nodeName of orderedNames) {
      const apiNode = activePipeline.nodes[nodeName];
      if (!apiNode) continue;

      // Resolve node position from various sources
      const { position: pos, fromPending: positionFromPending } = resolveNodePosition(
        nodeName,
        prevPositions,
        savedPositions
      );

      // Save position to staging store if it came from pending (newly dropped) and we're in staging mode
      if (positionFromPending && isInStagingMode && selectedSessionId) {
        updateNodePosition(selectedSessionId, nodeName, pos);
      }

      const nodeState = currentNodeStates[nodeName] || apiNode.state;

      // Determine if this node is staged (for visual distinction)
      const isStaged = isInStagingMode && (stagingData?.stagedNodes.has(nodeName) ?? false);

      // Get base pins from definition and resolve dynamic pins
      const baseInputs = defByKind.get(apiNode.kind)?.inputs ?? [];
      const baseOutputs = defByKind.get(apiNode.kind)?.outputs ?? [];
      const nodeDefinition = defByKind.get(apiNode.kind);

      const { finalInputs, finalOutputs } = resolveDynamicPins(
        nodeDefinition,
        nodeName,
        activePipeline,
        baseInputs,
        baseOutputs
      );

      const nodeDef = defByKind.get(apiNode.kind);

      // Build node object using helper function
      const node = buildNodeObject({
        nodeName,
        apiNode,
        position: pos,
        nodeState,
        isStaged,
        finalInputs,
        finalOutputs,
        nodeDef,
        stableOnParamChange,
        stableOnConfigChange,
        selectedSessionId,
      });

      newNodes.push(node);
    }

    // Build edges using helper function
    const newEdges = buildEdgesFromConnections(activePipeline.connections, newNodes);

    viewsLogger.debug('Setting', newNodes.length, 'nodes and', newEdges.length, 'edges');
    // Batch node and edge updates to prevent double render
    React.startTransition(() => {
      setNodes(newNodes);
      setEdges(newEdges);
      topoEffectRanRef.current = true;
    });

    // Generate YAML using helper function
    const generatedYaml = generatePipelineYaml(activePipeline, orderedNames);
    setYamlFromTopology(generatedYaml);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [topoKey, defByKind, selectedSessionId, tuneNode]);

  // Create a stable callback that handles both staged and live param changes
  // This avoids recreating callbacks for each node, which would break React.memo
  const stableOnParamChange = useCallback(
    (nodeId: string, paramName: string, value: unknown) => {
      // Check at call time if we're in staging mode and if this node is staged
      const currentStagingData = useStagingStore.getState().staging[selectedSessionId || ''];
      const isCurrentlyInStagingMode = currentStagingData?.mode === 'staging';
      const isNodeStaged = isCurrentlyInStagingMode && currentStagingData?.stagedNodes.has(nodeId);

      if (isNodeStaged && selectedSessionId) {
        updateStagedNodeParams(selectedSessionId, nodeId, { [paramName]: value });
        return;
      }

      // Validate before sending to server
      const error = validateParamValue(nodeId, paramName, value);
      if (error) {
        toast.error(`Invalid value for ${paramName}: ${error}`);
        return;
      }

      tuneNode(nodeId, paramName, value);
    },
    [selectedSessionId, updateStagedNodeParams, validateParamValue, toast, tuneNode]
  );

  // Stable callback for full-config updates (compositor nodes).
  // Supports staging mode: staged config changes are stored locally,
  // live changes are sent directly via tuneNodeConfig.
  const stableOnConfigChange = useCallback(
    (nodeId: string, config: Record<string, unknown>) => {
      const currentStagingData = useStagingStore.getState().staging[selectedSessionId || ''];
      const isCurrentlyInStagingMode = currentStagingData?.mode === 'staging';
      const isNodeStaged = isCurrentlyInStagingMode && currentStagingData?.stagedNodes.has(nodeId);

      if (isNodeStaged && selectedSessionId) {
        updateStagedNodeParams(selectedSessionId, nodeId, config);
        return;
      }

      tuneNodeConfig(nodeId, config);
    },
    [selectedSessionId, updateStagedNodeParams, tuneNodeConfig]
  );

  // In staging mode, keep each node's `isStaged` flag in sync with `stagedNodes`.
  useEffect(() => {
    if (nodes.length === 0 || !isInStagingMode) return;

    // Only update if there are actually staged nodes
    if (!stagingData?.stagedNodes || stagingData.stagedNodes.size === 0) return;

    viewsLogger.debug('Updating isStaged flags for', stagingData.stagedNodes.size, 'nodes');
    setNodes((prev) =>
      prev.map((n) => {
        const isNodeStaged = stagingData.stagedNodes.has(n.id);
        if (n.data.isStaged === isNodeStaged) {
          return n; // No change
        }
        return {
          ...n,
          data: {
            ...n.data,
            isStaged: isNodeStaged,
          },
        };
      })
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stagingData?.stagedNodes, nodes.length, isInStagingMode]);

  // NOTE: fitView is triggered only by:
  // 1. Auto-layout effect (when needsAutoLayout is true)
  // 2. needsFit effect (when needsFit is true)
  // Avoid auto-fitting on every node change to prevent disruption during editing.

  // YAML regeneration is handled by useMonitorYaml hook.

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

    if (isInStagingMode && selectedSessionId) {
      // Add to staging store
      addStagedNode(selectedSessionId, nodeId, {
        kind,
        params: params as Record<string, unknown>,
        state: null,
      });
    } else {
      // Send to server immediately (monitor mode)
      addNode(nodeId, kind, params);
    }

    setType(null);
  };

  // Memoize left panel to prevent ResizableLayout from re-rendering
  const leftPanel = React.useMemo(
    () => (
      <LeftPanel
        isLoadingSessions={isLoadingSessions}
        sessions={sessions}
        selectedSessionId={selectedSessionId}
        onSessionClick={handleSessionClick}
        onSessionDelete={handleQuickDeleteSession}
        editMode={editMode}
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
      editMode,
      nodeDefinitions,
      onDragStart,
      pluginKinds,
      pluginTypes,
    ]
  );

  // Memoize center panel to prevent ResizableLayout from re-rendering

  // - Only track nodes.length, not full nodes array (FlowCanvas handles position updates internally)
  // - Only track stagingData lengths, not full object (TopControls has its own memo optimization)
  // - Handlers are stable via refs and don't need to be tracked
  // - selectedSession used instead of sessions array to prevent unnecessary re-renders
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
            isInStagingMode={isInStagingMode}
            stagingData={stagingData}
            onCommit={handleCommitChanges}
            onDiscard={handleDiscardChanges}
            onEnterStaging={handleEnterStagingMode}
            onDelete={handleDeleteModalOpen}
            onStartPreview={handleStartPreview}
            isPreviewConnected={isPreviewConnected}
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
              editMode={editMode}
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
            {isLoadingPipeline ? (
              <p>Loading pipeline...</p>
            ) : (
              <p>Select a session from the left panel to inspect its pipeline.</p>
            )}
          </EmptyMonitorState>
        )}
        <OutputPreviewPanel hasSession={selectedSessionId != null} conditionalRender />
      </CenterPanelContainer>
    ),
    // Intentional sparse dependencies for performance optimization:
    // - Only track nodes.length, not full nodes array (FlowCanvas handles position updates internally)
    // - Only track stagingData lengths, not full object (TopControls has its own memo optimization)
    // - Handlers are stable via refs and don't need to be tracked
    // - selectedSession used instead of sessions array to prevent unnecessary re-renders
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [
      selectedSessionId,
      selectedSession,
      isConnected,
      isInStagingMode,
      stagingData?.changes.length,
      stagingData?.validationErrors.length,
      nodes.length,
      colorMode,
      onInit,
      editMode,
      isLoadingPipeline,
      handleStartPreview,
      isPreviewConnected,
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
          onYamlChange={isInStagingMode ? handleYamlChange : undefined}
          onParamChange={handleRightPaneParamChange}
          onLabelChange={handleRightPaneLabelChange}
          nodeDefinitions={nodeDefinitions}
          readOnly={!editMode}
          yamlReadOnly={!isInStagingMode}
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
      isInStagingMode,
      handleYamlChange,
      handleRightPaneParamChange,
      handleRightPaneLabelChange,
      nodeDefinitions,
      editMode,
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
        onCancel={() => setShowDeleteModal(false)}
        isLoading={isDeletingSession}
      />
      <ConfirmModal
        isOpen={sessionToDelete !== null}
        title="Delete Session"
        message={`Are you sure you want to delete session "${sessionToDelete}"? This will stop the pipeline and all running nodes. This action cannot be undone.`}
        confirmLabel="Delete"
        cancelLabel="Cancel"
        onConfirm={handleConfirmQuickDelete}
        onCancel={() => setSessionToDelete(null)}
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
