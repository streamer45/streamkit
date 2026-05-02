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
  JsonValue,
  NodeState,
  Pipeline,
  MessageType,
  InputPin,
  OutputPin,
} from '@/types/types';
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
import {
  buildEdgesFromConnections,
  buildNodeObject,
  generatePipelineYaml,
} from '@/utils/pipelineGraph';
import { nodeTypes, defaultEdgeOptions } from '@/utils/reactFlowDefaults';

// Memoized view title to prevent re-renders during drag
const MonitorViewTitle = React.memo(() => <ViewTitle>Monitor</ViewTitle>);

// UI-only node dropped on the canvas, promoted to a real node on
// explicit "Add to pipeline" click — never auto-promoted on edit.
export type DraftNode = {
  kind: string;
  params: Record<string, unknown>;
  position: { x: number; y: number };
  missingRequired: string[];
  /** True between "Add to pipeline" click and the engine's
   *  NodeAdded/Failed reply.  Cleared on Failed so the user can fix
   *  the input and click again. */
  inFlight?: boolean;
};

// Returns the failure reason for `NodeState::Failed`, else undefined.
const nodeStateFailedReason = (s: NodeState | null | undefined): string | undefined => {
  if (s && typeof s === 'object' && 'Failed' in s) {
    const f = (s as { Failed: { reason?: string } }).Failed;
    return f?.reason;
  }
  return undefined;
};

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

  // Defer 'dimensions' changes (post-mount measurement) via startTransition
  // so they don't block interactive changes.
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

  // Ensure plugins are loaded and schemas include plugin node definitions
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
  // Drafts to mark selected on their first topology rebuild; cleared
  // once applied so React Flow owns selection thereafter.
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

  // ── Draft nodes ───────────────────────────────────────────────────────
  // See module-level DraftNode type for the full lifecycle.
  const [draftNodes, setDraftNodes] = useState<Map<string, DraftNode>>(new Map());
  // Read-only snapshot for callbacks that mustn't depend on `draftNodes`.
  // Modifying paths use functional `setDraftNodes((prev) => ...)`.
  const draftNodesRef = useRef(draftNodes);
  useEffect(() => {
    draftNodesRef.current = draftNodes;
  }, [draftNodes]);
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

  // YAML by default; inspector opens on double-click — except drafts,
  // which need the inspector to fill required fields.
  useEffect(() => {
    if (selectedNodes.length === 1 && draftNodesRef.current.has(selectedNodes[0])) {
      setRightPaneView('inspector');
      return;
    }
    setRightPaneView('yaml');
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
    // Recompute only when meaningful data changes — not on every position update.
    if (
      !prev ||
      prev.id !== selectedNode.id ||
      prev.type !== selectedNode.type ||
      prevData?.['kind'] !== nextData['kind'] ||
      prevData?.['label'] !== nextData['label'] ||
      prevData?.['sessionId'] !== nextData['sessionId'] ||
      !deepEqual(prevData?.['state'], nextData['state']) ||
      !deepEqual(prevData?.['params'], nextData['params']) ||
      !deepEqual(prevData?.['draft'], nextData['draft'])
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

  // Subscribe to selected session.  Pipeline is fetched once and kept
  // current by live WS events — no polling.  nodeStates/nodeStats are
  // consumed from session store directly via NodeStateIndicator.
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

  // Dot-path-aware deep merge for nested params (vs useSession's shallow merge).
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

  // Drop drafts whose ids now appear in pipeline.nodes — the engine's
  // node-added forwarder only inserts after successful creation.
  // Depend on pipeline.nodes only and read the current draft snapshot
  // via the ref, so this doesn't re-run on every keystroke into a draft.
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

  // Drop one or more drafts: clear their per-node atom (so a re-dropped
  // draft with the same generated id doesn't inherit stale typed values)
  // and remove them from the drafts map in a single setDraftNodes commit.
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

  // Per-promotion failure subscriptions, set up by promoteDraft and
  // reaped here when the draft is dropped or its in-flight flag clears.
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

  // Discard drafts on session switch — they're tied to the previous
  // canvas's coords and namespace.  useLayoutEffect to clear before
  // paint so the topology effect doesn't briefly render them on the
  // new session's canvas.
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

  // Topology signature: only changes when nodes/kinds or connections change
  const topoKey = React.useMemo(() => {
    if (!pipeline && draftNodes.size === 0) return '';
    const names = pipeline ? Object.keys(pipeline.nodes).sort() : [];
    const kinds = names.map((n) => `${n}:${pipeline!.nodes[n].kind}`);
    const conns = pipeline
      ? pipeline.connections
          .map((c: Connection) => `${c.from_node}:${c.from_pin}>${c.to_node}:${c.to_pin}`)
          .sort()
      : [];
    // Track only schema KEYS (not content): runtime_param_schema is
    // documented as immutable for the node's lifetime.
    const runtimeKeys = Object.keys(pipeline?.runtime_schemas ?? {}).sort();
    // Draft fingerprint excludes param values — drafts render from
    // their Jotai atom on each keystroke, no rebuild needed.
    const draftFingerprint = Array.from(draftNodes.entries())
      .map(([id, d]) => `${id}:${d.kind}:${d.missingRequired.join(',')}:${d.inFlight ? '1' : '0'}`)
      .sort();
    const key = JSON.stringify([kinds, conns, runtimeKeys, draftFingerprint]);
    viewsLogger.debug('topoKey recalculated:', key.substring(0, 100));
    return key;
  }, [pipeline, draftNodes]);

  // Auto-layout + fit-view hook
  const { setNeedsAutoLayout, setNeedsFit, handleAutoLayout } = useAutoLayout({
    pipeline,
    selectedSessionId,
    nodesLength: nodes.length,
    setNodes,
    rf,
    updateNodePosition,
  });

  // Throttled Zustand→ReactFlow edge-alert patching bridge
  const { topoEffectRanRef } = useNodeStatesSubscription({
    selectedSessionId,
    setEdges,
    pipelineRef,
    topoKey,
  });

  // Eagerly clear selection when a previously-seen session vanishes
  // (destroyed).  The ref guards against clearing a freshly-set
  // session id (from nav state) that hasn't appeared in the list yet.
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

  // Validate against the runtime-merged schema if available.  Drafts
  // fall back to the static schema since the engine hasn't run them yet.
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

      // Direct lookup, then dot-path lookup via each property's `path`
      // field (used by runtime-discovered Slint properties).
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

  // Ref keeps stableOnParamChange identity stable across pipeline
  // changes — preserves React.memo on each node component.
  const validateParamValueRef = useRef(validateParamValue);
  validateParamValueRef.current = validateParamValue;

  // Update local draft state only.  Promotion is exclusively via the
  // "Add to pipeline" button, never as a side-effect of typing.
  const handleDraftParamChange = useCallback(
    (nodeId: string, key: string, value: unknown) => {
      // Required-but-empty is handled by computeMissingRequired (the
      // "needs ..." banner), not surfaced as a validation error here.
      const validationError = validateParamValueRef.current(nodeId, key, value);
      if (validationError) {
        toast.error(`Invalid value for ${key}: ${validationError}`);
        return;
      }
      // Mirror the edit into the per-node atom so InspectorPane sees
      // every keystroke without waiting on a render.
      if (key.includes('.')) {
        writeNodeParams(nodeId, buildParamUpdate(key, value), selectedSessionId ?? undefined);
      } else {
        writeNodeParam(nodeId, key, value, selectedSessionId ?? undefined);
      }

      // Functional updater: rapid edits to different keys each see the
      // previous one's commit.
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

  // The only place `addNode` fires for drafts (wired to the "Add to
  // pipeline" button on the draft banner).
  const promoteDraft = useCallback(
    (nodeId: string) => {
      const draft = draftNodesRef.current.get(nodeId);
      if (!draft) return;
      if (draft.inFlight) return;
      const missing = computeMissingRequired(draft.kind, draft.params, nodeDefinitions);
      if (missing.length > 0) return;

      // Subscribe before addNode so a synchronously-emitted Failed
      // (e.g. duplicate-id rejection) isn't missed.
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
        // sub() doesn't fire for the current value — check once in
        // case the atom is already populated (retry of a failed id).
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

  // Stable ref so onPromote arrows don't churn when promoteDraft re-creates.
  const stablePromoteDraftRef = useRef(promoteDraft);
  stablePromoteDraftRef.current = promoteDraft;

  // Memoized param change handler for right pane
  const handleRightPaneParamChange = useCallback(
    (nodeId: string, key: string, value: unknown) => {
      if (draftNodesRef.current.has(nodeId)) {
        handleDraftParamChange(nodeId, key, value);
        return;
      }
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
    [toast, tuneNode, tuneNodeConfigDeep, handleDraftParamChange]
  );

  // Memoized label change handler (currently no-op)
  const handleRightPaneLabelChange = useCallback(() => {}, []);

  // Memoized handlers for TopControls to prevent re-renders
  const handleDeleteModalOpen = useCallback(() => {
    setShowDeleteModal(true);
  }, []);

  const onConnect = React.useCallback(
    (connection: RFConnection) => {
      // Block connections that touch a draft — the node does not exist
      // in the engine yet, so a `connect` would fail with "Source/Target
      // node not found".  Steer the user to the inspector instead.
      const drafts = draftNodesRef.current;
      const sourceDraft = connection.source ? drafts.get(connection.source) : undefined;
      const targetDraft = connection.target ? drafts.get(connection.target) : undefined;
      if (sourceDraft || targetDraft) {
        const draft = sourceDraft ?? targetDraft!;
        const draftId = sourceDraft ? connection.source : connection.target;
        // missingRequired empties before nodeadded echoes; surface the
        // transitional in-flight state instead of an empty list.
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

  // Deletion is handled by React Flow's built-in delete key via onNodesDelete/onEdgesDelete.

  // Considers both live pipeline and in-flight drafts to avoid collisions.
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

  // Track previous topoKey to avoid unnecessary rebuilds
  const prevTopoKeyForTopologyRef = useRef<string>('');

  // Position lookup: prev render → persistent store → origin.  Drop
  // coordinates for new nodes are written to the store in `onDrop`.
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

    if (!pipeline && draftNodes.size === 0) {
      viewsLogger.debug('Topology effect: No pipeline, clearing nodes');
      setNodes([]);
      setEdges([]);
      setYamlString('');
      return;
    }

    viewsLogger.debug('Topology effect triggered, topoKey:', topoKey.substring(0, 50) + '...');

    // Preserve existing node positions; do not auto-layout during edits.
    const orderedNames: string[] = pipeline
      ? (() => {
          const { levels, sortedLevels } = topoLevelsFromPipeline(pipeline);
          return orderedNamesFromLevels(levels, sortedLevels);
        })()
      : [];

    const prevPositions = new Map(nodes.map((n) => [n.id, n.position]));
    // setNodes(newNodes) replaces the array, so `selected` is lost
    // unless we re-apply it.  Fresh drafts use newDraftSelectionRef.
    const prevSelected = new Set<string>(nodes.filter((n) => n.selected).map((n) => n.id));

    // Get saved positions from position store
    const savedPositions = selectedSessionId ? getNodePositions(selectedSessionId) : {};

    const newNodes: RFNode[] = [];
    for (const nodeName of orderedNames) {
      const apiNode = pipeline!.nodes[nodeName];
      if (!apiNode) continue;

      const { position: pos } = resolveNodePosition(nodeName, prevPositions, savedPositions);

      // Use real-time state from Jotai atom if available, otherwise use pipeline state.
      // Read directly from the default store (non-reactive) since the topology effect
      // only runs on structural changes, not on every node-state transition.
      const nodeState =
        (selectedSessionId
          ? defaultSessionStore.get(nodeStateAtom(nodeKey(selectedSessionId, nodeName)))
          : null) ?? apiNode.state;

      // Get base pins from definition and resolve dynamic pins
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

      // Merge runtime param schema (if any) with the static per-kind schema.
      // Runtime schemas are per-instance overrides discovered after node init
      // (e.g. Slint component properties enumerated from the compiled .slint).
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

    // Append draft nodes (UI-only, not yet sent to engine).
    for (const [draftId, draft] of draftNodes) {
      const draftDef = defByKind.get(draft.kind);
      const draftBaseInputs = draftDef?.inputs ?? [];
      const draftBaseOutputs = draftDef?.outputs ?? [];
      // No engine state yet → use template pins, skip dynamic-pin reconstruction.
      const draftFinalInputs = draftBaseInputs;
      const draftFinalOutputs = draftBaseOutputs;
      // Prefer prev/saved position over draft.position so topology
      // rebuilds don't snap back to the original drop point after a drag.
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
      // Just-dropped draft: mark selected so the inspector opens.
      if (newDraftSelectionRef.current.has(draftId)) {
        node.selected = true;
        newDraftSelectionRef.current.delete(draftId);
      }
      newNodes.push(node);
    }

    // Build edges using helper function (only from real pipeline
    // connections — drafts cannot be connected).
    const newEdges = buildEdgesFromConnections(pipeline?.connections ?? [], newNodes);

    // Re-apply the previous selected flag so React Flow's selection
    // state survives the topology rebuild (see prevSelected comment).
    for (const n of newNodes) {
      if (prevSelected.has(n.id)) n.selected = true;
    }

    viewsLogger.debug('Setting', newNodes.length, 'nodes and', newEdges.length, 'edges');
    // Batch node and edge updates to prevent double render
    React.startTransition(() => {
      setNodes(newNodes);
      setEdges(newEdges);
      topoEffectRanRef.current = true;
    });

    // Generate YAML using helper function (drafts are excluded from YAML
    // until they're committed — a draft has no real existence in the
    // engine yet).
    const yamlString = pipeline ? generatePipelineYaml(pipeline, orderedNames) : '';
    setYamlString(yamlString);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [topoKey, defByKind, selectedSessionId, tuneNode]);

  // Stable param-change callback: routes to draft state or to server.
  const stableOnParamChange = useCallback(
    (nodeId: string, paramName: string, value: unknown) => {
      if (draftNodesRef.current.has(nodeId)) {
        handleDraftParamChange(nodeId, paramName, value);
        return;
      }
      // Validate before sending to server
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

  // Keep YAML in sync with live param overrides; runs only on param changes.
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

    // Calculate drop position in flow coordinates
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
    setSessionToDelete(sessionId);
  }, []);

  const handleConfirmQuickDelete = useCallback(async () => {
    if (!sessionToDelete) return;

    setIsDeletingSession(true);

    try {
      // Only tear down preview if it's for the session being deleted —
      // sessionToDelete may be any session from the sidebar.
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
