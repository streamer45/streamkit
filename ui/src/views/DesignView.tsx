// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import '@xyflow/react/dist/style.css';
import '@/App.css';
import styled from '@emotion/styled';
import {
  ReactFlowProvider,
  useOnSelectionChange,
  type ReactFlowInstance,
  type Node as RFNode,
  type Connection,
  type Edge,
  type OnConnectEnd,
} from '@xyflow/react';
import React, { useState, useEffect, useRef } from 'react';
import { useNavigate } from 'react-router-dom';
import { useShallow } from 'zustand/shallow';

import ConfirmModal from '@/components/ConfirmModal';
import ContextMenu from '@/components/ContextMenu';
import CreateSessionModal from '@/components/CreateSessionModal';
import { FlowCanvas } from '@/components/FlowCanvas';
import PaneContextMenu from '@/components/PaneContextMenu';
import { PipelineRightPane, type RightPaneView } from '@/components/PipelineRightPane';
import { ResizableLayout } from '@/components/ResizableLayout';
import SaveFragmentModal from '@/components/SaveFragmentModal';
import SaveTemplateModal from '@/components/SaveTemplateModal';
import { SKTooltip } from '@/components/Tooltip';
import { Button } from '@/components/ui/Button';
import { ViewTitle } from '@/components/ui/ViewTitle';
import { DnDProvider, useDnD } from '@/context/DnDContext';
import { useToast } from '@/context/ToastContext';
import { useContextMenu } from '@/hooks/useContextMenu';
import { useDesignViewModals } from '@/hooks/useDesignViewModals';
import { useFitViewOnLayoutPresetChange } from '@/hooks/useFitViewOnLayoutPresetChange';
import { usePermissions } from '@/hooks/usePermissions';
import { usePipeline } from '@/hooks/usePipeline';
import { useReactFlowCommon } from '@/hooks/useReactFlowCommon';
import { useResolvedColorMode } from '@/hooks/useResolvedColorMode';
import ControlPane from '@/panes/ControlPane';
import type { SamplePipelinesPaneRef, FragmentSample } from '@/panes/SamplePipelinesPane';
import { saveFragment, yamlToFragment } from '@/services/fragments';
import { saveSample } from '@/services/samples';
import { createSession } from '@/services/sessions';
import { useLayoutStore } from '@/stores/layoutStore';
import type { NodeDefinition } from '@/types/generated/api-types';
import { topoLevelsFromEdges, verticalLayout } from '@/utils/dag';
import { deepEqual } from '@/utils/deepEqual';
import { extractFragment, fragmentToReactFlow } from '@/utils/fragmentUtils';
import {
  DEFAULT_NODE_WIDTH,
  DEFAULT_NODE_HEIGHT,
  DEFAULT_HORIZONTAL_GAP,
  DEFAULT_VERTICAL_GAP,
  ESTIMATED_HEIGHT_BY_KIND,
} from '@/utils/layoutConstants';
import { viewsLogger } from '@/utils/logger';
import { nodeTypes, defaultEdgeOptions } from '@/utils/reactFlowDefaults';
import { collectNodeHeights } from '@/utils/reactFlowInstance';

const AppContainer = styled.div`
  height: 100%;
`;

const CenterContainer = styled.div`
  width: 100%;
  height: 100%;
  position: relative;
`;

const CanvasTopBar = styled.div`
  position: absolute;
  top: 12px;
  left: 12px;
  right: 12px;
  z-index: 10;
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  pointer-events: none;

  @media (max-width: 900px) {
    flex-direction: column;
    align-items: stretch;
  }
`;

const CanvasTitle = styled(ViewTitle)`
  pointer-events: none;
`;

const TopRightControlsContainer = styled.div`
  display: flex;
  flex-wrap: wrap;
  justify-content: flex-end;
  gap: 8px;
  align-items: center;
  pointer-events: auto;

  @media (max-width: 900px) {
    justify-content: flex-start;
  }
`;

// Memoized ViewTitle to prevent re-renders during drag
const DesignViewTitle = React.memo(() => <CanvasTitle>Design</CanvasTitle>);

// Memoized TopRightControls component to prevent re-renders during drag
const TopRightControls = React.memo(
  ({
    mode,
    onModeChange,
    selectionMode,
    onSelectionModeChange,
    onClear,
    onSaveTemplate,
    onCreateSession,
    nodesLength,
  }: {
    mode: 'oneshot' | 'dynamic';
    onModeChange: () => void;
    selectionMode: boolean;
    onSelectionModeChange: () => void;
    onClear: () => void;
    onSaveTemplate: () => void;
    onCreateSession: () => void;
    nodesLength: number;
  }) => {
    const { can } = usePermissions();

    return (
      <TopRightControlsContainer>
        <SKTooltip
          content={
            mode === 'oneshot'
              ? 'Oneshot mode for file conversion workflows'
              : 'Switch to oneshot mode (your dynamic canvas is preserved)'
          }
        >
          <Button
            variant={mode === 'oneshot' ? 'primary' : 'secondary'}
            size="small"
            onClick={() => mode !== 'oneshot' && onModeChange()}
          >
            📄 Oneshot
          </Button>
        </SKTooltip>
        <SKTooltip
          content={
            mode === 'dynamic'
              ? 'Dynamic mode for real-time streaming pipelines'
              : 'Switch to dynamic mode (your oneshot canvas is preserved)'
          }
        >
          <Button
            variant={mode === 'dynamic' ? 'primary' : 'secondary'}
            size="small"
            onClick={() => mode !== 'dynamic' && onModeChange()}
          >
            ⚡ Dynamic
          </Button>
        </SKTooltip>
        <SKTooltip
          content={selectionMode ? 'Switch to connection mode' : 'Switch to selection mode'}
        >
          <Button
            variant="secondary"
            size="small"
            active={selectionMode}
            onClick={onSelectionModeChange}
          >
            {selectionMode ? '🖱️ Selection' : '🔗 Connection'}
          </Button>
        </SKTooltip>
        <SKTooltip
          content={
            nodesLength === 0 ? 'Canvas is already empty' : 'Clear all nodes and connections'
          }
        >
          <Button variant="ghost" size="small" onClick={onClear} disabled={nodesLength === 0}>
            🗑️ Clear
          </Button>
        </SKTooltip>
        <SKTooltip
          content={
            !can.saveTemplate
              ? 'You do not have permission to save templates'
              : nodesLength === 0
                ? 'Add nodes to save a template'
                : 'Save pipeline as a reusable template'
          }
        >
          <Button
            variant="primary"
            size="small"
            onClick={onSaveTemplate}
            disabled={nodesLength === 0 || !can.saveTemplate}
          >
            💾 Save Template
          </Button>
        </SKTooltip>
        {mode === 'dynamic' && (
          <SKTooltip
            content={
              !can.createSession
                ? 'You do not have permission to create sessions'
                : nodesLength === 0
                  ? 'Add nodes to create a session'
                  : 'Create and test session'
            }
          >
            <Button
              variant="primary"
              size="small"
              onClick={onCreateSession}
              disabled={nodesLength === 0 || !can.createSession}
            >
              ▶️ Create Session
            </Button>
          </SKTooltip>
        )}
      </TopRightControlsContainer>
    );
  }
);

interface JsonSchema {
  properties?: Record<string, { default?: unknown }>;
}

type EditorNodeData = {
  label: string;
  kind: string;
  params?: Record<string, unknown>;
  ui?: { position?: { x: number; y: number } };
  paramSchema?: unknown;
  inputs?: unknown;
  outputs?: unknown;
  nodeDefinition?: unknown;
  definition?: { bidirectional?: boolean };
  onParamChange?: (nodeId: string, paramName: string, value: unknown) => void;
  onLabelChange?: (nodeId: string, newLabel: string) => void;
};

type PipelineCanvasCache = {
  nodes: RFNode<EditorNodeData>[];
  edges: Edge[];
  name: string;
  description: string;
};

/**
 * Helper function to handle fragment drops
 */
function processFragmentDrop(
  fragmentDataStr: string,
  position: { x: number; y: number },
  filteredNodeDefinitions: NodeDefinition[],
  stableHandleParamChange: (nodeId: string, paramName: string, value: unknown) => void,
  stableHandleLabelChange: (nodeId: string, newLabel: string) => void,
  nextLabelForKind: (kind: string) => string,
  currentNodes: RFNode<EditorNodeData>[],
  currentEdges: Edge[],
  layoutFragmentNodes: (
    fragmentNodeIds: string[],
    allNodes: RFNode<EditorNodeData>[],
    allEdges: Edge[],
    dropPosition: { x: number; y: number }
  ) => RFNode<EditorNodeData>[]
): { nodes: RFNode<EditorNodeData>[]; edges: Edge[]; fragmentName: string } | null {
  try {
    const fragment = JSON.parse(fragmentDataStr) as FragmentSample;
    const { nodes: fragmentNodesData } = yamlToFragment(fragment.yaml);
    const { nodes: fragmentNodes, edges: fragmentEdges } = fragmentToReactFlow(
      { nodes: fragmentNodesData },
      position,
      filteredNodeDefinitions,
      {
        onParamChange: stableHandleParamChange,
        onLabelChange: stableHandleLabelChange,
      },
      nextLabelForKind
    );

    const fragmentNodeIds = fragmentNodes.map((n) => n.id);
    const combinedNodes = [...currentNodes, ...(fragmentNodes as RFNode<EditorNodeData>[])];
    const combinedEdges = [...currentEdges, ...fragmentEdges];
    const layoutedNodes = layoutFragmentNodes(
      fragmentNodeIds,
      combinedNodes,
      combinedEdges,
      position
    );

    return {
      nodes: layoutedNodes,
      edges: combinedEdges,
      fragmentName: fragment.name,
    };
  } catch (error) {
    viewsLogger.error('Failed to parse fragment data:', error);
    return null;
  }
}

/**
 * Helper function to handle audio asset drops
 */
function processAudioAssetDrop(
  assetPath: string,
  position: { x: number; y: number },
  filteredNodeDefinitions: NodeDefinition[],
  getId: () => string,
  nextLabelForKind: (kind: string) => string,
  stableHandleParamChange: (nodeId: string, paramName: string, value: unknown) => void,
  stableHandleLabelChange: (nodeId: string, newLabel: string) => void
) {
  const fileReaderDefinition = filteredNodeDefinitions.find(
    (def) => def.kind === 'core::file_reader'
  );
  const newId = getId();

  const newNode = {
    id: newId,
    type: 'configurable',
    dragHandle: '.drag-handle',
    position,
    data: {
      label: nextLabelForKind('core::file_reader'),
      kind: 'core::file_reader',
      params: {
        path: assetPath,
        chunk_size: 8192,
      },
      paramSchema: fileReaderDefinition?.param_schema,
      inputs: fileReaderDefinition?.inputs || [],
      outputs: fileReaderDefinition?.outputs || [],
      nodeDefinition: fileReaderDefinition,
      definition: { bidirectional: fileReaderDefinition?.bidirectional },
      onParamChange: stableHandleParamChange,
      onLabelChange: stableHandleLabelChange,
    },
    selected: true,
  };

  return { node: newNode, nodeId: newId };
}

/**
 * Helper function to handle regular node drops
 */
function processRegularNodeDrop(
  type: string,
  position: { x: number; y: number },
  filteredNodeDefinitions: NodeDefinition[],
  getId: () => string,
  nextLabelForKind: (kind: string) => string,
  stableHandleParamChange: (nodeId: string, paramName: string, value: unknown) => void,
  stableHandleLabelChange: (nodeId: string, newLabel: string) => void
) {
  const nodeDefinition = filteredNodeDefinitions.find((def) => def.kind === type);
  const newId = getId();

  let nodeType = 'configurable';
  let defaultParams: Record<string, unknown> = {};

  if (type === 'audio::gain') {
    nodeType = 'audioGain';
    defaultParams = { gain: 1.0 };
  } else if (
    nodeDefinition &&
    (nodeDefinition.param_schema as JsonSchema | undefined)?.properties
  ) {
    Object.entries(
      (nodeDefinition.param_schema as JsonSchema).properties! as Record<
        string,
        { default?: unknown }
      >
    ).forEach(([key, schema]) => {
      if (schema.default !== undefined) {
        defaultParams[key] = schema.default;
      }
    });
  }

  const newNode = {
    id: newId,
    type: nodeType,
    dragHandle: '.drag-handle',
    position,
    data: {
      label: nextLabelForKind(type),
      kind: type,
      params: defaultParams,
      paramSchema: nodeDefinition?.param_schema,
      inputs: nodeDefinition?.inputs || [],
      outputs: nodeDefinition?.outputs || [],
      nodeDefinition: nodeDefinition,
      definition: { bidirectional: nodeDefinition?.bidirectional },
      onParamChange: stableHandleParamChange,
      onLabelChange: stableHandleLabelChange,
    },
    selected: true,
  };

  return { node: newNode, nodeId: newId };
}

/**
 * Main DesignView component for the pipeline editor.
 *
 * This component manages the interactive node graph editor, including:
 * - React Flow canvas for node visualization and editing
 * - Drag-and-drop support for nodes, fragments, and assets
 * - Modal management for templates, sessions, and fragments
 * - Node selection and inspector functionality
 * - Auto-layout and graph manipulation
 *
 */
// eslint-disable-next-line max-statements -- Single-view editor orchestration
const DesignViewContent: React.FC = () => {
  const { menu, paneMenu, reactFlowWrapper, onNodeContextMenu, onPaneContextMenu, onPaneClick } =
    useContextMenu();
  const { rightCollapsed, setRightCollapsed } = useLayoutStore(
    useShallow((state) => ({
      rightCollapsed: state.rightCollapsed,
      setRightCollapsed: state.setRightCollapsed,
    }))
  );
  const [selectedNodes, setSelectedNodes] = useState<string[]>([]);
  const [rightPaneView, setRightPaneView] = useState<RightPaneView>('yaml');
  const [selectionMode, setSelectionMode] = useState(false);
  const {
    showClearModal,
    showSaveModal,
    showCreateModal,
    showLoadSampleModal,
    showSaveFragmentModal,
    pendingSample,
    setPendingSample,
    handleOpenClearModal,
    handleCloseClearModal,
    handleOpenSaveModal,
    handleCloseSaveModal,
    handleOpenCreateModal,
    handleCloseCreateModal,
    handleOpenLoadSampleModal,
    handleCloseLoadSampleModal,
    handleOpenSaveFragmentModal,
    handleCloseSaveFragmentModal,
  } = useDesignViewModals();
  const rf = useRef<ReactFlowInstance<RFNode<EditorNodeData>, Edge> | null>(null);
  const samplesRef = useRef<SamplePipelinesPaneRef | null>(null);
  const importFileInputRef = useRef<HTMLInputElement>(null);
  const onInit = React.useCallback((instance: ReactFlowInstance<RFNode<EditorNodeData>, Edge>) => {
    rf.current = instance;
  }, []);

  const screenToFlow = React.useCallback((pt: { x: number; y: number }) => {
    return rf.current?.screenToFlowPosition(pt) ?? pt;
  }, []);
  const [type, setType] = useDnD();
  const colorMode = useResolvedColorMode();
  const toast = useToast();
  const navigate = useNavigate();

  const {
    nodes,
    setNodes,
    onNodesChange,
    edges,
    setEdges,
    onEdgesChange,
    nodeDefinitions,
    yamlString,
    yamlError,
    mode,
    setMode,
    pipelineName,
    setPipelineName,
    pipelineDescription,
    setPipelineDescription,
    handleExportYaml,
    handleImportYaml,
    handleYamlChange,
    nextLabelForKind,
    handleParamChange,
    handleLabelChange,
    regenerateYamlFromCanvas,
    getId,
  } = usePipeline();

  const cachesRef = React.useRef<Partial<Record<'oneshot' | 'dynamic', PipelineCanvasCache>>>({});

  // Keep per-mode caches updated with the latest canvas state.
  React.useEffect(() => {
    cachesRef.current[mode] = {
      nodes,
      edges,
      name: pipelineName,
      description: pipelineDescription,
    };
  }, [mode, nodes, edges, pipelineName, pipelineDescription]);

  // Create stable callback refs for node data to prevent unnecessary re-renders
  const handleParamChangeRef = React.useRef(handleParamChange);
  const handleLabelChangeRef = React.useRef(handleLabelChange);
  React.useEffect(() => {
    handleParamChangeRef.current = handleParamChange;
    handleLabelChangeRef.current = handleLabelChange;
  }, [handleParamChange, handleLabelChange]);

  // Stable callbacks that never change identity
  const stableHandleParamChange = React.useCallback(
    (nodeId: string, paramName: string, value: unknown) => {
      handleParamChangeRef.current(nodeId, paramName, value);
    },
    []
  );

  const stableHandleLabelChange = React.useCallback((nodeId: string, newLabel: string) => {
    handleLabelChangeRef.current(nodeId, newLabel);
  }, []);

  const rehydrateNodesForCanvas = React.useCallback(
    (cachedNodes: RFNode<EditorNodeData>[]) => {
      return cachedNodes.map((n) => ({
        ...n,
        data: {
          ...n.data,
          onParamChange: stableHandleParamChange,
          onLabelChange: stableHandleLabelChange,
        },
      }));
    },
    [stableHandleParamChange, stableHandleLabelChange]
  );

  const {
    isValidConnection: isValidConnectionBase,
    createOnConnect,
    createOnConnectEnd,
  } = useReactFlowCommon();

  // Keep refs to avoid recreating callbacks on every drag
  const nodesRef = React.useRef(nodes);
  const edgesRef = React.useRef(edges);
  React.useEffect(() => {
    nodesRef.current = nodes;
    edgesRef.current = edges;
  }, [nodes, edges]);

  const handleNodeDragStop = React.useCallback(() => {
    // Keep YAML ordering in sync with the canvas (top-down) without regenerating on every drag tick.
    // Note: onNodeDragStop's third param only contains the dragged nodes, not all nodes.
    // Use nodesRef.current which has all nodes with updated positions from onNodesChange.
    regenerateYamlFromCanvas({ nodes: nodesRef.current, edges: edgesRef.current });
  }, [regenerateYamlFromCanvas]);

  // Wrap isValidConnection to pass current nodes and edges via refs
  const isValidConnectionWrapper = React.useCallback(
    (connection: Connection | Edge) => {
      return isValidConnectionBase(connection, nodesRef.current, edgesRef.current);
    },
    [isValidConnectionBase]
  );

  // Filter nodes based on mode - memoize to prevent left panel re-renders
  const filteredNodeDefinitions = React.useMemo(() => {
    return nodeDefinitions.filter((def) => {
      const isOneshotNode = def.categories.includes('oneshot');
      const isDynamicNode = def.categories.includes('dynamic');

      if (mode === 'oneshot') {
        // In oneshot mode, hide dynamic-only nodes
        return !isDynamicNode;
      } else {
        // In dynamic mode, hide oneshot-only nodes
        return !isOneshotNode;
      }
    });
  }, [nodeDefinitions, mode]);

  const onEdgesDelete = React.useCallback(
    (deleted: Edge[]) => {
      setEdges((eds) => eds.filter((e) => !deleted.some((del) => del.id === e.id)));

      // Remove dynamically created pins that no longer have connections
      deleted.forEach((edge) => {
        const targetNode = nodesRef.current.find((n) => n.id === edge.target);
        if (!targetNode) return;

        // Check if this node has dynamic pins
        const nodeDefinition = (targetNode.data as Record<string, unknown>)?.nodeDefinition as
          | { inputs?: unknown[] }
          | undefined;
        const hasDynamicInputs =
          nodeDefinition?.inputs?.some(
            (pin: unknown) =>
              typeof (pin as { cardinality?: unknown }).cardinality === 'object' &&
              'Dynamic' in ((pin as { cardinality?: unknown }).cardinality as object)
          ) ?? false;

        if (!hasDynamicInputs) return;

        // Find the pin that was connected
        const targetHandle = edge.targetHandle;
        if (!targetHandle) return;

        // Check if this pin has any other remaining connections
        const hasOtherConnections = edgesRef.current.some(
          (e) => e.target === edge.target && e.targetHandle === targetHandle && e.id !== edge.id
        );

        if (hasOtherConnections) return;

        // Remove the pin from the node
        setNodes((nds) =>
          nds.map((n) => {
            if (n.id === edge.target) {
              const inputs = (n.data.inputs || []) as unknown[];
              const filteredInputs = inputs.filter(
                (pin) => (pin as { name: string }).name !== targetHandle
              );

              return {
                ...n,
                data: {
                  ...n.data,
                  inputs: filteredInputs,
                },
              };
            }
            return n;
          })
        );
      });
    },
    [setEdges, setNodes]
  );

  const onNodesDelete = React.useCallback(
    (deleted: RFNode[]) => {
      const deletedIds = new Set(deleted.map((n) => n.id));
      setNodes((nds) => nds.filter((n) => !deletedIds.has(n.id)));
      setEdges((eds) => eds.filter((e) => !deletedIds.has(e.source) && !deletedIds.has(e.target)));
    },
    [setNodes, setEdges]
  );

  // Wrapper for setNodes to match the expected signature in createOnConnect
  const setNodesWrapper = React.useCallback(
    (updater: (nodes: RFNode<EditorNodeData>[]) => RFNode<EditorNodeData>[]) => {
      setNodes((prevNodes) => updater(prevNodes) as RFNode<EditorNodeData>[]);
    },
    [setNodes]
  );

  const onConnect = React.useCallback(
    (connection: Connection) => {
      return createOnConnect(
        nodesRef.current,
        setEdges,
        undefined,
        edgesRef.current,
        setNodesWrapper
      )(connection);
    },
    [createOnConnect, setEdges, setNodesWrapper]
  );

  const onConnectEnd: OnConnectEnd = React.useCallback(
    (event, connectionState) => {
      return createOnConnectEnd(nodesRef.current, edgesRef.current)(event, connectionState);
    },
    [createOnConnectEnd]
  );

  // Handle selection changes
  // Avoid re-render loops on pan/zoom: only update when selection actually changes
  const arraysEqual = (a: string[], b: string[]) =>
    a.length === b.length && a.every((v, i) => v === b[i]);

  useOnSelectionChange({
    onChange: ({ nodes: selectedNodes }) => {
      const nextNodeIds = selectedNodes.map((node) => node.id);
      setSelectedNodes((prev) => (arraysEqual(prev, nextNodeIds) ? prev : nextNodeIds));
    },
  });

  // Deletion is handled by React Flow's built-in delete key via onNodesDelete/onEdgesDelete.

  const handleAutoLayout = () => {
    if (nodes.length === 0) return;

    const nodeIds = nodes.map((n) => n.id);
    const { levels, sortedLevels } = topoLevelsFromEdges(
      nodeIds,
      edges.map((e) => ({ source: e.source, target: e.target }))
    );

    const measuredHeights = collectNodeHeights(rf.current);
    const perNodeHeights: Record<string, number> = {};

    nodes.forEach((n) => {
      const measured = measuredHeights[n.id];
      if (typeof measured === 'number' && Number.isFinite(measured)) {
        perNodeHeights[n.id] = measured;
        return;
      }

      const kind = (n.data as EditorNodeData).kind;
      perNodeHeights[n.id] = ESTIMATED_HEIGHT_BY_KIND[kind] ?? DEFAULT_NODE_HEIGHT;
    });

    const positions = verticalLayout(levels, sortedLevels, {
      nodeWidth: DEFAULT_NODE_WIDTH,
      nodeHeight: DEFAULT_NODE_HEIGHT,
      hGap: DEFAULT_HORIZONTAL_GAP,
      vGap: DEFAULT_VERTICAL_GAP,
      heights: perNodeHeights,
      edges: edges.map((e) => ({ source: e.source, target: e.target })),
    });

    const nextNodes = nodes.map((n) => ({
      ...n,
      position: positions[n.id] ?? n.position,
    }));

    setNodes(nextNodes);
    // Pass edges explicitly to avoid stale closure issues.
    regenerateYamlFromCanvas({ nodes: nextNodes, edges: edgesRef.current });

    setTimeout(() => {
      rf.current?.fitView({ padding: 0.2, duration: 300 });
    }, 0);
  };

  // Layout just the fragment nodes around a drop position
  const layoutFragmentNodes = React.useCallback(
    (
      fragmentNodeIds: string[],
      allNodes: RFNode<EditorNodeData>[],
      allEdges: Edge[],
      dropPosition: { x: number; y: number }
    ) => {
      if (fragmentNodeIds.length === 0) return allNodes;

      // Filter to only edges within the fragment
      const fragmentEdges = allEdges.filter(
        (e) => fragmentNodeIds.includes(e.source) && fragmentNodeIds.includes(e.target)
      );

      // Calculate layout for fragment nodes
      const { levels, sortedLevels } = topoLevelsFromEdges(
        fragmentNodeIds,
        fragmentEdges.map((e) => ({ source: e.source, target: e.target }))
      );

      const measuredHeights = collectNodeHeights(rf.current);
      const perNodeHeights: Record<string, number> = {};

      fragmentNodeIds.forEach((id) => {
        const node = allNodes.find((n) => n.id === id);
        if (!node) return;

        const measured = measuredHeights[id];
        if (typeof measured === 'number' && Number.isFinite(measured)) {
          perNodeHeights[id] = measured;
          return;
        }

        const kind = (node.data as EditorNodeData).kind;
        perNodeHeights[id] = ESTIMATED_HEIGHT_BY_KIND[kind] ?? DEFAULT_NODE_HEIGHT;
      });

      const positions = verticalLayout(levels, sortedLevels, {
        nodeWidth: DEFAULT_NODE_WIDTH,
        nodeHeight: DEFAULT_NODE_HEIGHT,
        hGap: DEFAULT_HORIZONTAL_GAP,
        vGap: DEFAULT_VERTICAL_GAP,
        heights: perNodeHeights,
        edges: fragmentEdges.map((e) => ({ source: e.source, target: e.target })),
      });

      // Offset positions to be relative to drop position
      return allNodes.map((n) => {
        if (!fragmentNodeIds.includes(n.id)) return n;

        const layoutPos = positions[n.id];
        if (!layoutPos) return n;

        return {
          ...n,
          position: {
            x: dropPosition.x + layoutPos.x,
            y: dropPosition.y + layoutPos.y,
          },
        };
      });
    },
    []
  );

  const handleSaveTemplate = async (name: string, description: string, overwrite = false) => {
    try {
      await saveSample({
        name,
        description,
        yaml: yamlString,
        overwrite,
        is_fragment: false,
      });

      toast.success(`Template "${name}" ${overwrite ? 'overwritten' : 'saved'} successfully!`);

      // Refresh the samples list
      samplesRef.current?.refresh();
    } catch (error) {
      viewsLogger.error('Failed to save template:', error);
      // Don't show toast for 409 conflict - let the modal handle it with the overwrite dialog
      if (!(error instanceof Error && error.message.includes('409'))) {
        toast.error(error instanceof Error ? error.message : 'Failed to save template');
      }
      throw error; // Re-throw so modal can handle it
    }
  };

  const handleCreateSession = async (name: string) => {
    try {
      // Validate pipeline has content before sending
      if (!yamlString.trim()) {
        throw new Error('Pipeline is empty. Add some nodes before creating a session.');
      }

      // Validate that we're in dynamic mode and don't have oneshot-only nodes
      if (mode === 'dynamic') {
        const oneshotNodes = nodes.filter((node) => {
          const nodeDef = nodeDefinitions.find((def) => def.kind === node.data.kind);
          return nodeDef?.categories.includes('oneshot');
        });

        if (oneshotNodes.length > 0) {
          const nodeList = oneshotNodes.map((n) => `"${n.data.label}" (${n.data.kind})`).join(', ');
          throw new Error(
            `Cannot create dynamic session: Pipeline contains oneshot-only nodes: ${nodeList}. ` +
              `These nodes are only compatible with oneshot mode. Please remove them or switch to oneshot mode.`
          );
        }
      }

      // Create session with pipeline using the sessions service
      const result = await createSession(name, yamlString);
      const sessionDisplayName = result.name || result.session_id;

      toast.success(`Session created: ${sessionDisplayName}`);
      toast.success('Pipeline deployed to session!');

      // Navigate to monitor view
      navigate('/monitor');
    } catch (error) {
      viewsLogger.error('Failed to create session:', error);
      toast.error(error instanceof Error ? error.message : 'Failed to create session');
      throw error; // Re-throw so modal can handle it
    }
  };

  const onDragOver = React.useCallback((event: React.DragEvent) => {
    event.preventDefault();
    event.dataTransfer.dropEffect = 'move';
  }, []);

  const onDrop = React.useCallback(
    (event: React.DragEvent) => {
      event.preventDefault();

      if (!type) {
        return;
      }

      const position = screenToFlow({
        x: event.clientX,
        y: event.clientY,
      });

      // Handle fragment drop
      if (type.startsWith('fragment:')) {
        const fragmentDataStr = event.dataTransfer.getData('application/x-streamkit-fragment');
        if (fragmentDataStr) {
          const result = processFragmentDrop(
            fragmentDataStr,
            position,
            filteredNodeDefinitions,
            stableHandleParamChange,
            stableHandleLabelChange,
            nextLabelForKind,
            nodesRef.current,
            edgesRef.current,
            layoutFragmentNodes
          );

          if (result) {
            setNodes(result.nodes);
            setEdges(result.edges);
            toast.success(`Added fragment: ${result.fragmentName}`);
          } else {
            toast.error('Failed to add fragment');
          }
        }
        return;
      }

      // Handle audio asset drop
      if (type.startsWith('audio-asset:')) {
        const assetPath = type.replace('audio-asset:', '');
        const { node: newNode, nodeId: newId } = processAudioAssetDrop(
          assetPath,
          position,
          filteredNodeDefinitions,
          getId,
          nextLabelForKind,
          stableHandleParamChange,
          stableHandleLabelChange
        );

        setNodes((nds) => nds.map((n) => ({ ...n, selected: false })).concat([newNode]));
        setSelectedNodes([newId]);

        if (rightCollapsed) {
          setRightCollapsed(false);
        }
        return;
      }

      // Handle regular node drop
      const { node: newNode, nodeId: newId } = processRegularNodeDrop(
        type,
        position,
        filteredNodeDefinitions,
        getId,
        nextLabelForKind,
        stableHandleParamChange,
        stableHandleLabelChange
      );

      setNodes((nds) => nds.map((n) => ({ ...n, selected: false })).concat([newNode]));
      setSelectedNodes([newId]);

      if (rightCollapsed) {
        setRightCollapsed(false);
      }
    },
    [
      type,
      screenToFlow,
      filteredNodeDefinitions,
      getId,
      nextLabelForKind,
      stableHandleParamChange,
      stableHandleLabelChange,
      setNodes,
      setEdges,
      setSelectedNodes,
      rightCollapsed,
      setRightCollapsed,
      toast,
      layoutFragmentNodes,
    ]
  );

  const onDragStart = React.useCallback(
    (event: React.DragEvent, nodeType: string) => {
      setType(nodeType);
      event.dataTransfer.setData('text/plain', nodeType);
      event.dataTransfer.effectAllowed = 'move';
    },
    [setType]
  );

  const onAssetDragStart = React.useCallback(
    (event: React.DragEvent, asset: import('@/types/generated/api-types').AudioAsset) => {
      const dragType = `audio-asset:${asset.path}`;
      setType(dragType);
      event.dataTransfer.setData('application/x-streamkit-audio-asset', JSON.stringify(asset));
      event.dataTransfer.effectAllowed = 'move';
    },
    [setType]
  );

  // Fragment drag and drop handlers
  const onFragmentDragStart = React.useCallback(
    (event: React.DragEvent, fragment: FragmentSample) => {
      const dragType = `fragment:${fragment.id}`;
      setType(dragType);
      event.dataTransfer.setData('application/x-streamkit-fragment', JSON.stringify(fragment));
      event.dataTransfer.effectAllowed = 'move';
    },
    [setType]
  );

  const handleFragmentInsert = React.useCallback(
    (fragment: FragmentSample) => {
      if (!rf.current) return;

      // Get the center of the viewport
      const viewport = rf.current.getViewport();
      const centerX = -viewport.x + window.innerWidth / 2 / viewport.zoom;
      const centerY = -viewport.y + window.innerHeight / 2 / viewport.zoom;
      const dropPosition = { x: centerX - 100, y: centerY - 100 };

      // Parse fragment from YAML
      const { nodes: fragmentNodesData } = yamlToFragment(fragment.yaml);

      const { nodes: fragmentNodes, edges: fragmentEdges } = fragmentToReactFlow(
        { nodes: fragmentNodesData },
        dropPosition,
        nodeDefinitions,
        {
          onParamChange: stableHandleParamChange,
          onLabelChange: stableHandleLabelChange,
        },
        nextLabelForKind
      );

      // Apply auto-layout to fragment nodes
      const fragmentNodeIds = fragmentNodes.map((n) => n.id);
      const combinedNodes = [...nodesRef.current, ...(fragmentNodes as RFNode<EditorNodeData>[])];
      const combinedEdges = [...edgesRef.current, ...fragmentEdges];
      const layoutedNodes = layoutFragmentNodes(
        fragmentNodeIds,
        combinedNodes,
        combinedEdges,
        dropPosition
      );

      setNodes(layoutedNodes);
      setEdges(combinedEdges);
      toast.success(`Added fragment: ${fragment.name}`);
    },
    [
      nodeDefinitions,
      stableHandleParamChange,
      stableHandleLabelChange,
      nextLabelForKind,
      setNodes,
      setEdges,
      toast,
      layoutFragmentNodes,
    ]
  );

  const handleSaveFragment = React.useCallback(
    async (name: string, description: string, tags: string[]) => {
      if (selectedNodes.length === 0) {
        toast.error('Please select nodes to save as a fragment');
        return;
      }

      const fragmentData = extractFragment(selectedNodes, nodesRef.current, edgesRef.current);

      try {
        await saveFragment(name, description, tags, fragmentData.nodes);
        toast.success(`Fragment "${name}" saved successfully`);
        handleCloseSaveFragmentModal();

        // Refresh the samples list to show the new fragment
        samplesRef.current?.refresh();
      } catch (error) {
        viewsLogger.error('Failed to save fragment:', error);
        toast.error(error instanceof Error ? error.message : 'Failed to save fragment');
      }
    },
    [selectedNodes, toast, handleCloseSaveFragmentModal]
  );

  const handleDuplicateNode = React.useCallback(
    (nodeId: string) => {
      const nodeToDuplicate = nodesRef.current.find((n) => n.id === nodeId);
      if (!nodeToDuplicate) return;

      const newId = getId();
      const newNode = {
        ...nodeToDuplicate,
        id: newId,
        dragHandle: '.drag-handle',
        position: {
          x: nodeToDuplicate.position.x,
          y: nodeToDuplicate.position.y + 150,
        },
        data: {
          ...nodeToDuplicate.data,
          label: nextLabelForKind(nodeToDuplicate.data.kind),
          onParamChange: stableHandleParamChange,
          onLabelChange: stableHandleLabelChange,
        },
        selected: false,
      };

      setNodes((nds) => nds.concat(newNode));
    },
    [getId, nextLabelForKind, setNodes, stableHandleParamChange, stableHandleLabelChange]
  );

  const handleDeleteNode = (nodeId: string) => {
    // Remove the node
    setNodes((nds) => nds.filter((node) => node.id !== nodeId));

    // Remove edges connected to the deleted node
    setEdges((eds) => eds.filter((edge) => edge.source !== nodeId && edge.target !== nodeId));
  };

  // Memoize selectedNode with custom comparison (ignore position changes)
  const selectedNodeId = selectedNodes.length === 1 ? selectedNodes[0] : null;
  const selectedNode = React.useMemo(() => {
    if (!selectedNodeId) return null;
    return nodes.find((node) => node.id === selectedNodeId) ?? null;
  }, [selectedNodeId, nodes]);

  // Create a stable reference for selectedNode that only changes when data (not position) changes
  const selectedNodeRef = React.useRef(selectedNode);
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
      !deepEqual(prevData?.['state'], nextData['state']) ||
      !deepEqual(prevData?.['params'], nextData['params'])
    ) {
      selectedNodeRef.current = selectedNode;
    }
    return selectedNodeRef.current;
  }, [selectedNode]);

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

  // Keep a ref to handleAutoLayout to avoid dependency issues
  const handleAutoLayoutRef = React.useRef(handleAutoLayout);
  React.useEffect(() => {
    handleAutoLayoutRef.current = handleAutoLayout;
  });

  // Run auto-layout after file imports, or fitView on initial page load.
  // We no longer fitView on every node add/remove as it's disruptive to manual placement.
  useEffect(() => {
    if (nodes.length > 0) {
      if (pendingAutoLayoutRef.current) {
        const timeoutId = window.setTimeout(() => {
          pendingAutoLayoutRef.current = false;
          handleAutoLayoutRef.current();
        }, 50);
        return () => window.clearTimeout(timeoutId);
      } else if (pendingInitialFitViewRef.current) {
        const timeoutId = window.setTimeout(() => {
          pendingInitialFitViewRef.current = false;
          rf.current?.fitView({ padding: 0.2, duration: 300 });
        }, 50);
        return () => window.clearTimeout(timeoutId);
      }
    }
  }, [nodes.length]);

  // Register fitView callback for layout preset changes
  useFitViewOnLayoutPresetChange({
    reactFlowInstance: rf,
    nodesCount: nodes.length,
  });

  const selectedNodeDefinition = (() => {
    if (!selectedNode) return null;
    return nodeDefinitions.find((def) => def.kind === selectedNode.data.kind) || null;
  })();

  // Memoized button handlers to prevent TopRightControls from re-rendering
  const handleModeToggle = React.useCallback(() => {
    const nextMode = mode === 'oneshot' ? 'dynamic' : 'oneshot';

    // Ensure the current canvas is cached even if the user switches immediately after load.
    cachesRef.current[mode] = {
      nodes: nodesRef.current,
      edges: edgesRef.current,
      name: pipelineName,
      description: pipelineDescription,
    };

    // Swap canvases instead of forcing users to clear incompatible nodes.
    // Keep a separate "last state" cache per mode.
    const nextCache = cachesRef.current[nextMode];
    const nextNodes = nextCache ? rehydrateNodesForCanvas(nextCache.nodes) : [];
    const nextEdges = nextCache?.edges ?? [];

    setMode(nextMode);
    setNodes(nextNodes);
    setEdges(nextEdges);
    setPipelineName(nextCache?.name ?? '');
    setPipelineDescription(nextCache?.description ?? '');
    setSelectedNodes([]);
    regenerateYamlFromCanvas({ nodes: nextNodes, edges: nextEdges, mode: nextMode });
  }, [
    mode,
    setMode,
    setNodes,
    setEdges,
    setPipelineName,
    setPipelineDescription,
    pipelineName,
    pipelineDescription,
    regenerateYamlFromCanvas,
    rehydrateNodesForCanvas,
  ]);

  const handleSelectionModeToggle = React.useCallback(() => {
    setSelectionMode(!selectionMode);
  }, [selectionMode]);

  // Use refs to avoid recreating callback on every drag
  const handleImportYamlRef = React.useRef(handleImportYaml);
  React.useEffect(() => {
    handleImportYamlRef.current = handleImportYaml;
  }, [handleImportYaml]);

  // Track when we need to auto-layout after import
  const pendingAutoLayoutRef = React.useRef(false);
  // Track initial page load to run fitView once when nodes are restored from localStorage
  const pendingInitialFitViewRef = React.useRef(true);

  // File import handlers - kept at DesignView level so file input survives menu close
  const handleImportFileChange = React.useCallback((event: React.ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    if (file) {
      const reader = new FileReader();
      reader.onload = (e) => {
        const content = e.target?.result as string;
        if (content) {
          pendingAutoLayoutRef.current = true;
          handleImportYamlRef.current(content);
        }
        // Reset input so same file can be selected again
        if (importFileInputRef.current) {
          importFileInputRef.current.value = '';
        }
      };
      reader.readAsText(file);
    }
  }, []);

  const triggerImportYaml = React.useCallback(() => {
    importFileInputRef.current?.click();
  }, []);

  const toastRef = React.useRef(toast);
  React.useEffect(() => {
    toastRef.current = toast;
  }, [toast]);

  const handleLoadSample = React.useCallback(
    (yamlString: string, name: string, description: string) => {
      // If canvas has nodes, show confirmation
      if (nodesRef.current.length > 0) {
        setPendingSample({ yaml: yamlString, name, description });
        handleOpenLoadSampleModal();
      } else {
        // Canvas is empty, load directly with auto-layout
        pendingAutoLayoutRef.current = true;
        handleImportYamlRef.current(yamlString, description, name);
        toastRef.current.success(`Loaded sample: ${name}`);
      }
    },
    [setPendingSample, handleOpenLoadSampleModal]
  );

  const confirmLoadSample = () => {
    if (pendingSample) {
      pendingAutoLayoutRef.current = true;
      handleImportYaml(pendingSample.yaml, pendingSample.description, pendingSample.name);
      toast.success(`Loaded sample: ${pendingSample.name}`);
      handleCloseLoadSampleModal();
    }
  };

  const handleClearCanvas = React.useCallback(() => {
    setNodes([]);
    setEdges([]);
    handleCloseClearModal();
    toast.success('Canvas cleared');
  }, [setNodes, setEdges, toast, handleCloseClearModal]);

  // Memoize left panel to prevent re-renders during drag
  const leftPanel = React.useMemo(
    () => (
      <ControlPane
        nodeDefinitions={filteredNodeDefinitions}
        onDragStart={onDragStart}
        onAssetDragStart={onAssetDragStart}
        onLoadSample={handleLoadSample}
        samplesRef={samplesRef}
        mode={mode}
        onFragmentDragStart={onFragmentDragStart}
        onFragmentInsert={handleFragmentInsert}
      />
    ),
    [
      filteredNodeDefinitions,
      onDragStart,
      onAssetDragStart,
      handleLoadSample,
      mode,
      onFragmentDragStart,
      handleFragmentInsert,
    ]
  );

  // Center panel contains React Flow which handles its own efficient updates
  // The parent (DesignViewContent) WILL re-render when nodes/edges change, and that's expected
  // What matters is that:
  // 1. Individual node components don't re-render (handled by React.memo)
  // 2. Callbacks are stable (handled by useCallback with refs)
  // 3. Sibling panels (left/right) don't re-render (they're memoized separately)
  const centerPanel = (
    <CenterContainer className="react-flow-container">
      <CanvasTopBar>
        <DesignViewTitle />
        <TopRightControls
          mode={mode}
          onModeChange={handleModeToggle}
          selectionMode={selectionMode}
          onSelectionModeChange={handleSelectionModeToggle}
          onClear={handleOpenClearModal}
          onSaveTemplate={handleOpenSaveModal}
          onCreateSession={handleOpenCreateModal}
          nodesLength={nodes.length}
        />
      </CanvasTopBar>
      <FlowCanvas
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        onNodesChange={onNodesChange}
        onEdgesChange={onEdgesChange}
        colorMode={colorMode}
        onInit={onInit}
        defaultEdgeOptions={defaultEdgeOptions}
        editMode={!selectionMode}
        selectionMode={selectionMode}
        isValidConnection={isValidConnectionWrapper}
        onConnect={onConnect}
        onConnectEnd={onConnectEnd}
        onEdgesDelete={onEdgesDelete}
        onNodesDelete={onNodesDelete}
        onNodeDoubleClick={handleNodeDoubleClick}
        onPaneClick={onPaneClick}
        onPaneContextMenu={onPaneContextMenu}
        onNodeContextMenu={onNodeContextMenu}
        onNodeDragStop={handleNodeDragStop}
        onDrop={onDrop}
        onDragOver={onDragOver}
        reactFlowWrapper={reactFlowWrapper}
      />
    </CenterContainer>
  );

  // Extract selected node label for YAML highlighting
  const selectedNodeLabel = React.useMemo(() => {
    return stableSelectedNode?.data?.label;
  }, [stableSelectedNode]);

  // Memoize right panel to prevent re-renders during drag
  const rightPanel = React.useMemo(
    () => (
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
        yamlError={yamlError}
        onYamlChange={handleYamlChange}
        onParamChange={stableHandleParamChange}
        onLabelChange={stableHandleLabelChange}
        nodeDefinitions={nodeDefinitions}
      />
    ),
    [
      stableSelectedNode,
      selectedNodeDefinition,
      selectedNodeLabel,
      rightPaneView,
      setRightPaneView,
      yamlString,
      yamlError,
      handleYamlChange,
      stableHandleParamChange,
      stableHandleLabelChange,
      nodeDefinitions,
    ]
  );

  return (
    <AppContainer className="app-container">
      <ResizableLayout
        left={leftPanel}
        center={centerPanel}
        right={rightPanel}
        leftLabel="Library"
        centerLabel="Canvas"
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
        <PaneContextMenu
          onClick={onPaneClick}
          onImportYaml={triggerImportYaml}
          onExportYaml={handleExportYaml}
          onAutoLayout={handleAutoLayout}
          onSaveFragment={() => {
            // Delay modal opening to allow context menu to fully close
            setTimeout(() => handleOpenSaveFragmentModal(), 0);
          }}
          hasSelectedNodes={selectedNodes.length > 0}
          {...paneMenu}
        />
      )}
      {/* Hidden file input for YAML import - lives here so it doesn't unmount with context menu */}
      <input
        type="file"
        ref={importFileInputRef}
        onChange={handleImportFileChange}
        accept=".yaml,.yml"
        style={{ display: 'none' }}
      />
      <ConfirmModal
        isOpen={showClearModal}
        title="Clear Canvas"
        message="Are you sure you want to clear the canvas? This will remove all nodes and connections. This action cannot be undone."
        confirmLabel="Clear"
        cancelLabel="Cancel"
        onConfirm={handleClearCanvas}
        onCancel={handleCloseClearModal}
      />
      <SaveTemplateModal
        isOpen={showSaveModal}
        onClose={handleCloseSaveModal}
        onSave={handleSaveTemplate}
        mode={mode}
        initialName={pipelineName}
        initialDescription={pipelineDescription}
      />
      <SaveFragmentModal
        isOpen={showSaveFragmentModal}
        onClose={handleCloseSaveFragmentModal}
        onSave={handleSaveFragment}
      />
      <CreateSessionModal
        isOpen={showCreateModal}
        onClose={handleCloseCreateModal}
        onCreate={handleCreateSession}
        mode={mode}
      />
      <ConfirmModal
        isOpen={showLoadSampleModal}
        title="Load Sample Pipeline"
        message={`Loading "${pendingSample?.name}" will replace your current canvas. Any unsaved changes will be lost. Do you want to continue?`}
        confirmLabel="Load Sample"
        cancelLabel="Cancel"
        onConfirm={confirmLoadSample}
        onCancel={handleCloseLoadSampleModal}
      />
    </AppContainer>
  );
};

const DesignView: React.FC = () => {
  return (
    <ReactFlowProvider>
      <DnDProvider>
        <DesignViewContent />
      </DnDProvider>
    </ReactFlowProvider>
  );
};

export default DesignView;
