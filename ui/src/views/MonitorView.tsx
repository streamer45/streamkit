// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import {
  ReactFlowProvider,
  useNodesState,
  useEdgesState,
  type Node as RFNode,
  type Edge,
  type NodeChange,
  type Connection as RFConnection,
  type ReactFlowInstance,
  type OnConnectEnd,
  type OnSelectionChangeFunc,
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
import { useReactFlowCommon } from '@/hooks/useReactFlowCommon';
import { useResolvedColorMode } from '@/hooks/useResolvedColorMode';
import { useSession } from '@/hooks/useSession';
import { useSessionList } from '@/hooks/useSessionList';
import { useTuneNode } from '@/hooks/useTuneNode';
import { useWebSocket } from '@/hooks/useWebSocket';
import { getWebSocketService } from '@/services/websocket';
import { useLayoutStore } from '@/stores/layoutStore';
import { useNodePositionStore } from '@/stores/nodePositionStore';
import { ensurePluginsLoaded, usePluginStore } from '@/stores/pluginStore';
import { useSchemaStore, syncPluginSchemas } from '@/stores/schemaStore';
import {
  sessionStore as defaultSessionStore,
  nodeStateAtom,
  nodeParamsAtom,
  nodeKey,
  clearNodeParams,
  writeNodeParam,
  writeNodeParams,
} from '@/stores/sessionAtoms';
import { useSessionStore } from '@/stores/sessionStore';
import type {
  NodeDefinition,
  Connection,
  SessionInfo,
  JsonValue,
  NodeState,
  Pipeline,
  MessageType,
  InputPin,
  OutputPin,
} from '@/types/types';
import { arraysEqual } from '@/utils/arraysEqual';
import { buildParamUpdate, dispatchParamUpdate } from '@/utils/controlProps';
import { topoLevelsFromPipeline, orderedNamesFromLevels } from '@/utils/dag';
import { deepEqual } from '@/utils/deepEqual';
import {
  computeMissingRequired,
  defaultParamsForKind as draftDefaultParamsForKind,
  mergeDraftParam,
} from '@/utils/draftNodes';
import { deepMergeSchemas, validateValue } from '@/utils/jsonSchema';
import type { JsonSchema, JsonSchemaProperty } from '@/utils/jsonSchema';
import { viewsLogger } from '@/utils/logger';
import { buildMonitorTopologyKey } from '@/utils/monitorTopology';
import {
  buildEdgesFromConnections,
  buildNodeObject,
  generatePipelineYaml,
} from '@/utils/pipelineGraph';
import { nodeTypes, defaultEdgeOptions } from '@/utils/reactFlowDefaults';

const MonitorViewTitle = () => <ViewTitle>Monitor</ViewTitle>;

export type DraftNode = {
  kind: string;
  params: Record<string, unknown>;
  position: { x: number; y: number };
  missingRequired: string[];
  inFlight?: boolean;
};

const nodeStateFailedReason = (s: NodeState | null | undefined): string | undefined => {
  if (s && typeof s === 'object' && 'Failed' in s) {
    const f = (s as { Failed: { reason?: string } }).Failed;
    return f?.reason;
  }
  return undefined;
};

/** Keep the previous node reference unless a right-pane-relevant field changed,
 *  so inspector consumers behind React.memo don't re-render on every drag. */
function pickStableNode(prev: RFNode | null, next: RFNode | null): RFNode | null {
  if (!next) return null;
  if (!prev || prev.id !== next.id || prev.type !== next.type) return next;
  const prevData = prev.data as Record<string, unknown>;
  const nextData = next.data as Record<string, unknown>;
  if (
    prevData['kind'] !== nextData['kind'] ||
    prevData['label'] !== nextData['label'] ||
    prevData['sessionId'] !== nextData['sessionId'] ||
    !deepEqual(prevData['state'], nextData['state']) ||
    !deepEqual(prevData['params'], nextData['params']) ||
    !deepEqual(prevData['draft'], nextData['draft'])
  ) {
    return next;
  }
  return prev;
}

/** Drag-move frames recreate node objects that differ only in position; keep
 *  the previous array reference in that case so the FlowCanvas element stays
 *  referentially stable (React Flow tracks drag positions internally). The
 *  position skip applies only while the node is dragging: programmatic moves
 *  (e.g. auto-layout) must propagate or fitView fits a stale viewport. Every
 *  other field (data, selected, measured, ...) must be compared: ReactFlow is
 *  controlled, so feeding it a node array that drops such a change makes it
 *  fight its own internal state. */
function nodeEqualIgnoringDragPosition(prev: RFNode, next: RFNode): boolean {
  const keys = new Set([...Object.keys(prev), ...Object.keys(next)] as Array<keyof RFNode>);
  const dragging = prev.dragging === true || next.dragging === true;
  for (const key of keys) {
    if (key === 'position' && dragging) continue;
    if (prev[key] !== next[key]) return false;
  }
  return true;
}

function pickStableCanvasNodes(prev: RFNode[], next: RFNode[]): RFNode[] {
  if (prev === next) return prev;
  if (prev.length !== next.length) return next;
  for (let i = 0; i < next.length; i++) {
    if (!nodeEqualIgnoringDragPosition(prev[i], next[i])) return next;
  }
  return prev;
}

/** Session list refetches recreate session objects; keep the previous
 *  reference when the rendered fields are unchanged. */
function pickStableSession(
  prev: SessionInfo | undefined,
  next: SessionInfo | undefined
): SessionInfo | undefined {
  if (!next) return undefined;
  if (
    !prev ||
    prev.id !== next.id ||
    prev.name !== next.name ||
    prev.created_at !== next.created_at
  ) {
    return next;
  }
  return prev;
}

// eslint-disable-next-line max-statements -- Main view component with many hooks and state management
const MonitorViewContent: React.FC = () => {
  const location = useLocation();
  const [selectedSessionId, setSelectedSessionId] = useState<string | null>(null);
  const [nodes, setNodes, onNodesChangeInternal] = useNodesState<RFNode>([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>([]);

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

  React.useEffect(() => {
    ensurePluginsLoaded()
      .then(() => {
        const kinds = usePluginStore.getState().plugins.map((p) => p.kind);
        return syncPluginSchemas(kinds).catch((err) => {
          viewsLogger.error('Failed to sync plugin schemas', err);
        });
      })
      .catch((err) => {
        viewsLogger.error('Failed to load plugins', err);
      });
  }, []);

  const updateNodePosition = useNodePositionStore((s) => s.updateNodePosition);
  const getNodePositions = useNodePositionStore((s) => s.getNodePositions);
  const clearSessionPositions = useNodePositionStore((s) => s.clearSession);

  const onNodeDragStop = useCallback(
    (_event: MouseEvent | TouchEvent, node: RFNode) => {
      if (selectedSessionId) {
        updateNodePosition(selectedSessionId, node.id, node.position);
      }
    },
    [selectedSessionId, updateNodePosition]
  );
  const [selectedNodes, setSelectedNodes] = useState<string[]>([]);
  const newDraftSelectionRef = useRef<Set<string>>(new Set());
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

  const [draftNodes, setDraftNodes] = useState<Map<string, DraftNode>>(new Map());
  const draftNodesRef = useRef(draftNodes);
  useEffect(() => {
    draftNodesRef.current = draftNodes;
  }, [draftNodes]);
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

  const nodesRefForCallbacks = React.useRef(nodes);
  const edgesRefForCallbacks = React.useRef(edges);
  React.useEffect(() => {
    nodesRefForCallbacks.current = nodes;
    edgesRefForCallbacks.current = edges;
  }, [nodes, edges]);

  const { menu, paneMenu, reactFlowWrapper, onNodeContextMenu, onPaneContextMenu, onPaneClick } =
    useContextMenu();

  // Passed as the `onSelectionChange` prop on <ReactFlow> (via FlowCanvas) rather than
  // registered through useOnSelectionChange: the prop is re-applied on every render, so it
  // survives the canvas remount that MonitorView triggers when session data arrives. The
  // hook's effect keys on the (compiler-stabilized) callback identity and would not re-run to
  // re-register after xyflow resets its store on that remount.
  const handleSelectionChange = useCallback<OnSelectionChangeFunc<RFNode>>(
    ({ nodes: selNodes }) => {
      const nextIds = selNodes.map((n) => n.id);
      setSelectedNodes((prev) => (arraysEqual(prev, nextIds) ? prev : nextIds));
    },
    []
  );

  useEffect(() => {
    if (selectedNodes.length === 1 && draftNodesRef.current.has(selectedNodes[0])) {
      setRightPaneView('inspector');
      return;
    }
    setRightPaneView('yaml');
  }, [selectedNodes]);

  const handleNodeDoubleClick = React.useCallback(() => {
    setRightPaneView('inspector');
    if (rightCollapsed) {
      setRightCollapsed(false);
    }
  }, [rightCollapsed, setRightCollapsed]);

  const selectedNodeId = selectedNodes.length === 1 ? selectedNodes[0] : null;
  const selectedNode = React.useMemo(() => {
    if (!selectedNodeId) return null;
    return nodes.find((node) => node.id === selectedNodeId) ?? null;
  }, [selectedNodeId, nodes]);

  // Render-time state adjustment (instead of a keyed memo) so the React
  // Compiler can optimize this component while keeping referential stability.
  const [cachedStableNode, setCachedStableNode] = useState(selectedNode);
  const stableSelectedNode = pickStableNode(cachedStableNode, selectedNode);
  if (stableSelectedNode !== cachedStableNode) {
    setCachedStableNode(stableSelectedNode);
  }

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

  const { isConnected: globalIsConnected } = useWebSocket();

  const { data: sessions = [], isLoading: isLoadingSessions } = useSessionList();

  useEffect(() => {
    const store = useSessionStore.getState();
    for (const s of sessions) {
      if (!store.getSession(s.id)) {
        store.initSession(s.id, true);
      }
    }
  }, [sessions]);

  const rawSelectedSession = React.useMemo(
    () => sessions.find((s) => s.id === selectedSessionId),
    [sessions, selectedSessionId]
  );

  const [cachedCanvasNodes, setCachedCanvasNodes] = useState(nodes);
  const canvasNodes = pickStableCanvasNodes(cachedCanvasNodes, nodes);
  if (canvasNodes !== cachedCanvasNodes) {
    setCachedCanvasNodes(canvasNodes);
  }

  const [cachedStableSession, setCachedStableSession] = useState(rawSelectedSession);
  const selectedSession = pickStableSession(cachedStableSession, rawSelectedSession);
  if (selectedSession !== cachedStableSession) {
    setCachedStableSession(selectedSession);
  }

  const {
    pipeline,
    isConnected: sessionIsConnected,
    tuneNode,
    tuneNodeConfig,
    addNode,
    removeNode,
    connectPins,
    disconnectPins,
  } = useSession(selectedSessionId);

  const { tuneNodeConfig: tuneNodeConfigDeep } = useTuneNode(selectedSessionId);

  const isConnected = selectedSessionId ? sessionIsConnected : globalIsConnected;

  const {
    isPreviewConnected,
    isPreviewLoading,
    previewError,
    handleStartPreview,
    handleStopPreview,
  } = useMonitorPreview(selectedSessionId);

  useEffect(() => {
    const drafts = draftNodesRef.current;
    if (drafts.size === 0) return;
    let changed = false;
    const next = new Map(drafts);
    for (const id of drafts.keys()) {
      if (!pipeline?.nodes[id]) continue;
      next.delete(id);
      changed = true;
    }
    if (changed) setDraftNodes(next);
  }, [pipeline?.nodes]);

  const deleteDrafts = useCallback(
    (ids: string[]) => {
      if (ids.length === 0) return;
      for (const id of ids) clearNodeParams(id, selectedSessionId ?? undefined);
      setDraftNodes((prev) => {
        const next = new Map(prev);
        for (const id of ids) next.delete(id);
        return next;
      });
    },
    [selectedSessionId]
  );

  const failureUnsubsRef = useRef<Map<string, () => void>>(new Map());
  useEffect(() => {
    for (const [id, unsub] of failureUnsubsRef.current) {
      const d = draftNodes.get(id);
      if (!d || !d.inFlight) {
        unsub();
        failureUnsubsRef.current.delete(id);
      }
    }
  }, [draftNodes]);
  useEffect(
    () => () => {
      for (const unsub of failureUnsubsRef.current.values()) unsub();
      failureUnsubsRef.current.clear();
    },
    []
  );

  const prevDraftSessionIdRef = useRef<string | null>(null);
  React.useLayoutEffect(() => {
    const prevSession = prevDraftSessionIdRef.current;
    prevDraftSessionIdRef.current = selectedSessionId;
    if (prevSession === null || prevSession === selectedSessionId) return;
    for (const id of draftNodesRef.current.keys()) {
      clearNodeParams(id, prevSession);
    }
    setDraftNodes((prev) => (prev.size === 0 ? prev : new Map()));
  }, [selectedSessionId]);

  const topoKey = React.useMemo(() => {
    if (!pipeline && draftNodes.size === 0) return '';
    const names = pipeline ? Object.keys(pipeline.nodes).sort() : [];
    const kinds = names.map((n) => `${n}:${pipeline!.nodes[n].kind}`);
    const conns = pipeline
      ? pipeline.connections
          .map((c: Connection) => `${c.from_node}:${c.from_pin}>${c.to_node}:${c.to_pin}`)
          .sort()
      : [];

    const runtimeKeys = Object.keys(pipeline?.runtime_schemas ?? {}).sort();

    const draftFingerprint = Array.from(draftNodes.entries())
      .map(([id, d]) => `${id}:${d.kind}:${d.missingRequired.join(',')}:${d.inFlight ? '1' : '0'}`)
      .sort();
    const topologyFingerprint = JSON.stringify([kinds, conns, runtimeKeys, draftFingerprint]);
    const key = buildMonitorTopologyKey(selectedSessionId, topologyFingerprint);
    viewsLogger.debug('topoKey recalculated:', key.substring(0, 100));
    return key;
  }, [pipeline, draftNodes, selectedSessionId]);

  const { setNeedsAutoLayout, setNeedsFit, handleAutoLayout } = useAutoLayout({
    pipeline,
    selectedSessionId,
    nodesLength: nodes.length,
    setNodes,
    rf,
    updateNodePosition,
  });

  const locationState = location.state as { sessionId?: string } | null;
  useEffect(() => {
    if (locationState?.sessionId && !selectedSessionId) {
      const sessionId = locationState.sessionId;
      const savedPos = getNodePositions(sessionId);
      const hasPositions = Object.keys(savedPos).length > 0;

      React.startTransition(() => {
        setSelectedSessionId(sessionId);
        setNeedsAutoLayout(!hasPositions);
        setNeedsFit(true);
      });

      window.history.replaceState({}, document.title);
    }
  }, [locationState, selectedSessionId, getNodePositions, setNeedsAutoLayout, setNeedsFit]);

  useEffect(() => {
    if (!selectedSessionId && !isLoadingSessions && sessions.length > 0) {
      const sessionId = sessions[0].id;
      const savedPos = getNodePositions(sessionId);

      React.startTransition(() => {
        setSelectedSessionId(sessionId);
        setNeedsAutoLayout(Object.keys(savedPos).length === 0);
        setNeedsFit(true);
      });
    }
  }, [
    selectedSessionId,
    isLoadingSessions,
    sessions,
    getNodePositions,
    setNeedsAutoLayout,
    setNeedsFit,
  ]);

  const sessionSeenInListRef = useRef(false);
  useEffect(() => {
    if (selectedSession) {
      sessionSeenInListRef.current = true;
    }
  }, [selectedSession]);
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

  const validateParamValue = useCallback(
    (nodeId: string, paramKey: string, value: unknown): string | null => {
      const node = pipeline?.nodes[nodeId];
      const draft = draftNodesRef.current.get(nodeId);
      const kind = node?.kind ?? draft?.kind;
      if (!kind) return null;

      const nodeDef = nodeDefinitions.find((d) => d.kind === kind);
      if (!nodeDef) return null;

      const runtimeSchema = pipeline?.runtime_schemas?.[nodeId] as JsonSchema | undefined;
      const baseSchema = nodeDef.param_schema as JsonSchema | undefined;
      const merged = runtimeSchema ? deepMergeSchemas(baseSchema, runtimeSchema) : baseSchema;
      if (!merged?.properties) return null;

      let propSchema = merged.properties[paramKey] as JsonSchemaProperty | undefined;
      if (!propSchema && paramKey.includes('.')) {
        for (const entry of Object.values(merged.properties)) {
          if (entry && (entry as JsonSchemaProperty).path === paramKey) {
            propSchema = entry as JsonSchemaProperty;
            break;
          }
        }
      }

      if (!propSchema) return null;

      return validateValue(value, propSchema);
    },
    [pipeline, nodeDefinitions]
  );

  const validateParamValueRef = useRef(validateParamValue);
  useEffect(() => {
    validateParamValueRef.current = validateParamValue;
  }, [validateParamValue]);

  const handleDraftParamChange = useCallback(
    (nodeId: string, key: string, value: unknown) => {
      const validationError = validateParamValueRef.current(nodeId, key, value);
      if (validationError) {
        toast.error(`Invalid value for ${key}: ${validationError}`);
        return;
      }
      if (key.includes('.')) {
        writeNodeParams(nodeId, buildParamUpdate(key, value), selectedSessionId ?? undefined);
      } else {
        writeNodeParam(nodeId, key, value, selectedSessionId ?? undefined);
      }

      setDraftNodes((prev) => {
        const c = prev.get(nodeId);
        if (!c || c.inFlight) return prev;
        const newParams = mergeDraftParam(c.params, key, value);
        const missing = computeMissingRequired(c.kind, newParams, nodeDefinitions);
        const next = new Map(prev);
        next.set(nodeId, { ...c, params: newParams, missingRequired: missing });
        return next;
      });
    },
    [nodeDefinitions, selectedSessionId, toast]
  );

  const promoteDraft = useCallback(
    (nodeId: string) => {
      const draft = draftNodesRef.current.get(nodeId);
      if (!draft) return;
      if (draft.inFlight) return;
      const missing = computeMissingRequired(draft.kind, draft.params, nodeDefinitions);
      if (missing.length > 0) return;

      if (selectedSessionId) {
        const stateAtom = nodeStateAtom(nodeKey(selectedSessionId, nodeId));
        const handle = () => {
          const reason = nodeStateFailedReason(defaultSessionStore.get(stateAtom));
          if (reason === undefined) return;
          const unsubExisting = failureUnsubsRef.current.get(nodeId);
          if (unsubExisting) {
            unsubExisting();
            failureUnsubsRef.current.delete(nodeId);
          }
          removeNode(nodeId);
          setDraftNodes((prev) => {
            const c = prev.get(nodeId);
            if (!c || !c.inFlight) return prev;
            const next = new Map(prev);
            next.set(nodeId, {
              ...c,
              missingRequired: computeMissingRequired(c.kind, c.params, nodeDefinitions),
              inFlight: false,
            });
            return next;
          });
          toast.error(`${nodeId} failed: ${reason}`);
        };
        const unsub = defaultSessionStore.sub(stateAtom, handle);
        const prior = failureUnsubsRef.current.get(nodeId);
        if (prior) prior();
        failureUnsubsRef.current.set(nodeId, unsub);
        handle();
      }

      addNode(nodeId, draft.kind, draft.params);
      setDraftNodes((prev) => {
        const c = prev.get(nodeId);
        if (!c) return prev;
        const next = new Map(prev);
        next.set(nodeId, {
          ...c,
          missingRequired: [],
          inFlight: true,
        });
        return next;
      });
    },
    [addNode, nodeDefinitions, selectedSessionId, removeNode, toast]
  );

  const stablePromoteDraftRef = useRef(promoteDraft);
  useEffect(() => {
    stablePromoteDraftRef.current = promoteDraft;
  }, [promoteDraft]);

  const handleRightPaneParamChange = useCallback(
    (nodeId: string, key: string, value: unknown) => {
      if (draftNodesRef.current.has(nodeId)) {
        handleDraftParamChange(nodeId, key, value);
        return;
      }
      const error = validateParamValueRef.current(nodeId, key, value);
      if (error) {
        toast.error(`Invalid value for ${key}: ${error}`);
        return;
      }

      dispatchParamUpdate(nodeId, key, value, tuneNode, tuneNodeConfigDeep);
    },
    [toast, tuneNode, tuneNodeConfigDeep, handleDraftParamChange]
  );

  const handleRightPaneLabelChange = useCallback(() => {}, []);

  const handleDeleteModalOpen = useCallback(() => {
    setShowDeleteModal(true);
  }, []);

  const onConnect = React.useCallback(
    (connection: RFConnection) => {
      const drafts = draftNodesRef.current;
      const sourceDraft = connection.source ? drafts.get(connection.source) : undefined;
      const targetDraft = connection.target ? drafts.get(connection.target) : undefined;
      if (sourceDraft || targetDraft) {
        const draft = sourceDraft ?? targetDraft!;
        const draftId = sourceDraft ? connection.source : connection.target;
        const message =
          draft.missingRequired.length > 0
            ? `Configure ${draft.missingRequired.join(', ')} on ${draftId} before connecting`
            : `${draftId} is being added to the pipeline — try again in a moment`;
        toast.error(message);
        return;
      }
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
    [createOnConnect, setEdges, connectPins, setNodes, toast]
  );

  const onEdgesDelete = (deleted: Edge[]) => {
    deleted.forEach((e) => {
      const from_pin = e.sourceHandle || 'out';
      const to_pin = e.targetHandle || 'in';
      disconnectPins(e.source, from_pin, e.target, to_pin);
    });
  };

  const onNodesDelete = (deleted: RFNode[]) => {
    const draftIds: string[] = [];
    for (const n of deleted) {
      if (draftNodesRef.current.has(n.id)) {
        draftIds.push(n.id);
      } else {
        removeNode(n.id);
      }
    }
    deleteDrafts(draftIds);
  };

  const generateName = (kind: string) => {
    const existing = new Set<string>(pipeline ? Object.keys(pipeline.nodes) : []);
    for (const id of draftNodesRef.current.keys()) existing.add(id);
    let i = 1;
    let candidate = `${kind}_${i}`;
    while (existing.has(candidate)) {
      i += 1;
      candidate = `${kind}_${i}`;
    }
    return candidate;
  };

  const onDragStart = useCallback(
    (event: React.DragEvent, nodeType: string) => {
      setType(nodeType);
      event.dataTransfer.setData('text/plain', nodeType);
      event.dataTransfer.effectAllowed = 'move';
    },
    [setType]
  );

  const prevTopoKeyForTopologyRef = useRef<string>('');

  const resolveNodePosition = useCallback(
    (
      nodeName: string,
      prevPositions: Map<string, { x: number; y: number }>,
      savedPositions: Record<string, { x: number; y: number }>
    ): { position: { x: number; y: number } } => {
      const pos = prevPositions.get(nodeName) ?? savedPositions[nodeName];
      return { position: pos ?? { x: 0, y: 0 } };
    },
    []
  );

  const reconstructDynamicInputs = useCallback(
    (
      nodeName: string,
      dynamicTemplate: InputPin,
      activePipeline: Pipeline,
      prevNode: RFNode | undefined
    ): InputPin[] => {
      const dynamicPins = new Map<string, InputPin>();

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

  const reconstructDynamicOutputs = useCallback(
    (
      nodeName: string,
      dynamicTemplate: OutputPin,
      activePipeline: Pipeline,
      prevNode: RFNode | undefined
    ): OutputPin[] => {
      const dynamicPins = new Map<string, OutputPin>();

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

  const stableOnParamChange = useCallback(
    (nodeId: string, paramName: string, value: unknown) => {
      if (draftNodesRef.current.has(nodeId)) {
        handleDraftParamChange(nodeId, paramName, value);
        return;
      }
      const error = validateParamValueRef.current(nodeId, paramName, value);
      if (error) {
        toast.error(`Invalid value for ${paramName}: ${error}`);
        return;
      }

      // dispatchParamUpdate handles nested dot-paths via tuneNodeConfigDeep.
      dispatchParamUpdate(nodeId, paramName, value, tuneNode, tuneNodeConfigDeep);
    },
    [toast, tuneNode, tuneNodeConfigDeep, handleDraftParamChange]
  );

  // Stable callback for full-config updates (compositor nodes).
  const stableOnConfigChange = useCallback(
    (nodeId: string, config: Record<string, unknown>) => {
      tuneNodeConfig(nodeId, config);
    },
    [tuneNodeConfig]
  );

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

    if (prevTopoKeyForTopologyRef.current === topoKey && nodes.length > 0) {
      viewsLogger.debug('Skipping topology effect, topoKey unchanged');
      return;
    }
    prevTopoKeyForTopologyRef.current = topoKey;

    if (!pipeline && draftNodes.size === 0) {
      viewsLogger.debug('Topology effect: No pipeline, clearing nodes');
      React.startTransition(() => {
        setNodes((prev) => (prev.length === 0 ? prev : []));
        setEdges((prev) => (prev.length === 0 ? prev : []));
        setYamlString('');
      });
      return;
    }

    viewsLogger.debug('Topology effect triggered, topoKey:', topoKey.substring(0, 50) + '...');

    const orderedNames: string[] = pipeline
      ? (() => {
          const { levels, sortedLevels } = topoLevelsFromPipeline(pipeline);
          return orderedNamesFromLevels(levels, sortedLevels);
        })()
      : [];

    const prevPositions = new Map(nodes.map((n) => [n.id, n.position]));
    const prevSelected = new Set<string>(nodes.filter((n) => n.selected).map((n) => n.id));

    const savedPositions = selectedSessionId ? getNodePositions(selectedSessionId) : {};

    const newNodes: RFNode[] = [];
    for (const nodeName of orderedNames) {
      const apiNode = pipeline!.nodes[nodeName];
      if (!apiNode) continue;

      const { position: pos } = resolveNodePosition(nodeName, prevPositions, savedPositions);

      const nodeState =
        (selectedSessionId
          ? defaultSessionStore.get(nodeStateAtom(nodeKey(selectedSessionId, nodeName)))
          : null) ?? apiNode.state;

      const nodeDef = defByKind.get(apiNode.kind);
      const baseInputs = nodeDef?.inputs ?? [];
      const baseOutputs = nodeDef?.outputs ?? [];

      const { finalInputs, finalOutputs } = resolveDynamicPins(
        nodeDef,
        nodeName,
        pipeline!,
        baseInputs,
        baseOutputs
      );

      const runtimeSchema = pipeline!.runtime_schemas?.[nodeName] as JsonSchema | undefined;
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

    for (const [draftId, draft] of draftNodes) {
      const draftDef = defByKind.get(draft.kind);
      const draftBaseInputs = draftDef?.inputs ?? [];
      const draftBaseOutputs = draftDef?.outputs ?? [];
      const draftFinalInputs = draftBaseInputs;
      const draftFinalOutputs = draftBaseOutputs;
      const draftPos = prevPositions.get(draftId) ?? savedPositions[draftId] ?? draft.position;
      const node = buildNodeObject({
        nodeName: draftId,
        apiNode: {
          kind: draft.kind,
          params: draft.params as JsonValue,
          state: null,
        },
        position: draftPos,
        nodeState: undefined,
        finalInputs: draftFinalInputs,
        finalOutputs: draftFinalOutputs,
        nodeDef: draftDef,
        stableOnParamChange,
        stableOnConfigChange,
        selectedSessionId,
        draft: {
          missingRequired: draft.missingRequired,
          isCreating: !!draft.inFlight,
          onPromote: () => stablePromoteDraftRef.current(draftId),
        },
      });
      if (newDraftSelectionRef.current.has(draftId)) {
        node.selected = true;
        newDraftSelectionRef.current.delete(draftId);
      }
      newNodes.push(node);
    }

    const newEdges = buildEdgesFromConnections(
      pipeline?.connections ?? [],
      newNodes,
      selectedSessionId && pipeline
        ? { sessionId: selectedSessionId, connections: pipeline.connections }
        : undefined
    );

    for (const n of newNodes) {
      if (prevSelected.has(n.id)) n.selected = true;
    }

    viewsLogger.debug('Setting', newNodes.length, 'nodes and', newEdges.length, 'edges');
    React.startTransition(() => {
      setNodes((prev) => (prev.length === 0 && newNodes.length === 0 ? prev : newNodes));
      setEdges((prev) => (prev.length === 0 && newEdges.length === 0 ? prev : newEdges));
    });

    const yamlString = pipeline ? generatePipelineYaml(pipeline, orderedNames) : '';
    React.startTransition(() => {
      setYamlString(yamlString);
    });
    // The topoKey ref guard makes re-runs from non-topology dep changes no-ops.
  }, [
    topoKey,
    nodes,
    pipeline,
    draftNodes,
    selectedSessionId,
    getNodePositions,
    defByKind,
    resolveNodePosition,
    resolveDynamicPins,
    stableOnParamChange,
    stableOnConfigChange,
    setNodes,
    setEdges,
  ]);

  // Keep YAML in sync with live param overrides; runs only on param changes.
  useEffect(() => {
    if (!pipeline) {
      React.startTransition(() => {
        setYamlString('');
      });
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

    const nextYaml = dump(yamlObject, { skipInvalid: true });
    React.startTransition(() => {
      setYamlString(nextYaml);
    });
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
    viewsLogger.debug('Duplicate node:', nodeId);
  };

  const handleDeleteNode = (nodeId: string) => {
    if (draftNodesRef.current.has(nodeId)) {
      deleteDrafts([nodeId]);
      return;
    }
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

    const position = screenToFlow({
      x: event.clientX,
      y: event.clientY,
    });

    const kind = type;
    const nodeId = generateName(kind);
    const params = draftDefaultParamsForKind(kind, nodeDefinitions);

    // Hold as a local draft if any required param has no default —
    // avoids round-tripping a guaranteed-to-fail `addnode`.
    const missing = computeMissingRequired(kind, params, nodeDefinitions);
    if (missing.length > 0) {
      setDraftNodes((prev) => {
        const next = new Map(prev);
        next.set(nodeId, { kind, params, position, missingRequired: missing });
        return next;
      });
      newDraftSelectionRef.current.add(nodeId);
      setRightPaneView('inspector');
      if (rightCollapsed) {
        setRightCollapsed(false);
      }
      toast.info(`Configure ${missing.join(', ')} before this node is added to the pipeline`);
    } else {
      // Persist the drop coordinate so the post-`nodeadded` topology
      // rebuild renders the new node where the user dropped it.
      if (selectedSessionId) {
        updateNodePosition(selectedSessionId, nodeId, position);
      }
      addNode(nodeId, kind, params);
    }

    setType(null);
  };

  const handleSessionClick = useCallback(
    (sessionId: string) => {
      // Use startTransition to make session loading non-blocking
      // This allows the UI to stay responsive while loading heavy pipelines
      React.startTransition(() => {
        setSelectedSessionId(sessionId);
        const savedPos = getNodePositions(sessionId);
        const hasPositions = Object.keys(savedPos).length > 0;

        viewsLogger.debug('Session click, hasPositions:', hasPositions);
        setNeedsAutoLayout(!hasPositions);
        setNeedsFit(true);
      });
    },
    [getNodePositions, setNeedsAutoLayout, setNeedsFit]
  );

  const handleQuickDeleteSession = useCallback((sessionId: string) => {
    setSessionToDelete(sessionId);
  }, []);

  const deleteSession = useCallback(
    async (
      targetId: string,
      { tearDownPreview, onSuccess }: { tearDownPreview: boolean; onSuccess: () => void }
    ) => {
      setIsDeletingSession(true);

      // Only the awaits live in the try: the React Compiler cannot optimize
      // components containing value blocks (ternary/logical) inside try/catch.
      let response;
      try {
        // Tear down preview/MoQ connection BEFORE destroying the session
        // to avoid SIGSEGV from WebCodecs operating on a dead stream.
        if (tearDownPreview) {
          await handleStopPreview();
        }

        const wsService = getWebSocketService();

        response = await wsService.send({
          type: 'request' as MessageType,
          correlation_id: uuidv4(),
          payload: {
            action: 'destroysession' as const,
            session_id: targetId,
          },
        });
      } catch (error) {
        viewsLogger.error('Failed to delete session:', error);
        let message = 'Failed to delete session';
        if (error instanceof Error) {
          message = error.message;
        }
        toast.error(message);
        setIsDeletingSession(false);
        return;
      }

      if (response.payload.action === 'sessiondestroyed') {
        toast.success(`Session ${targetId} deleted successfully`);
        clearSessionPositions(targetId);
        onSuccess();
      } else if (response.payload.action === 'error') {
        viewsLogger.error('Failed to delete session:', response.payload.message);
        toast.error(response.payload.message || 'Failed to delete session');
      }
      setIsDeletingSession(false);
    },
    [toast, clearSessionPositions, handleStopPreview]
  );

  const handleConfirmQuickDelete = useCallback(async () => {
    if (!sessionToDelete) return;
    // Only tear down preview if it's for the session being deleted —
    // sessionToDelete may be any session from the sidebar.
    await deleteSession(sessionToDelete, {
      tearDownPreview: sessionToDelete === selectedSessionId,
      onSuccess: () => {
        if (selectedSessionId === sessionToDelete) {
          setSelectedSessionId(null);
        }
        setSessionToDelete(null);
      },
    });
  }, [sessionToDelete, selectedSessionId, deleteSession]);

  const handleDeleteSession = useCallback(async () => {
    if (!selectedSessionId) return;
    await deleteSession(selectedSessionId, {
      tearDownPreview: true,
      onSuccess: () => {
        setSelectedSessionId(null);
        setShowDeleteModal(false);
      },
    });
  }, [selectedSessionId, deleteSession]);

  const handleCancelDeleteModal = useCallback(() => setShowDeleteModal(false), []);
  const handleCancelQuickDelete = useCallback(() => setSessionToDelete(null), []);

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

  const centerPanel = (
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
            nodes={canvasNodes}
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
            onSelectionChange={handleSelectionChange}
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
  );

  const selectedNodeLabel = React.useMemo(() => {
    return stableSelectedNode?.data?.label as string | undefined;
  }, [stableSelectedNode]);

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
