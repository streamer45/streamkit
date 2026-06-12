// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useNodesState, useEdgesState, type Node, type Edge } from '@xyflow/react';
import { dump } from 'js-yaml';
import { useState, useEffect, useRef, useCallback, useMemo } from 'react';

import { useToast } from '@/context/ToastContext';
import { useSchemaStore } from '@/stores/schemaStore';
import {
  sessionStore as defaultSessionStore,
  nodeParamsAtom,
  writeNodeParam,
  writeNodeParams,
  clearNodeParams,
} from '@/stores/sessionAtoms';
import type { NodeDefinition } from '@/types/types';
import { dispatchParamUpdate } from '@/utils/controlProps';
import { hooksLogger } from '@/utils/logger';
import { parseYamlToPipeline, type EngineMode } from '@/utils/yamlPipeline';

import { buildPipelineForYaml } from './pipelineBuilder';
import type { EditorNodeData, ConnectionMode } from './pipelineBuilder';

const LOCAL_STORAGE_KEY = 'sk-pipeline-draft';

let id = 1;
const getId = () => `skitnode_${id++}`;

type DraftNodeData = Record<string, unknown> & {
  label: string;
  kind: string;
  params?: Record<string, unknown>;
  ui?: { position?: { x: number; y: number } };
};

type YamlSnapshot = {
  nodes?: Array<Node<EditorNodeData>>;
  edges?: Array<Edge>;
  mode?: EngineMode;
};

interface PipelineDraft {
  nodes: Node<DraftNodeData>[];
  edges: Edge[];
  mode?: EngineMode;
  name?: string;
  description?: string;
  labelCounters: Record<string, number>;
}

// Lives outside the hook so hydration stays synchronous (no flash of an
// empty canvas) and the module-level id counter can be advanced without
// tripping the React Compiler.
function loadDraftFromStorage(): PipelineDraft | null {
  try {
    const item = window.localStorage.getItem(LOCAL_STORAGE_KEY);
    if (!item) return null;

    const {
      nodes: savedNodes,
      edges: savedEdges,
      mode: savedMode,
      name: savedName,
      description: savedDescription,
    } = JSON.parse(item) as {
      nodes: Node<DraftNodeData>[];
      edges: Edge[];
      mode?: EngineMode;
      name?: string;
      description?: string;
    };

    if (!Array.isArray(savedNodes) || !Array.isArray(savedEdges)) return null;

    let maxId = 0;
    savedNodes.forEach((node) => {
      const match = node.id.match(/^skitnode_(\\d+)$/);
      if (match) {
        const num = parseInt(match[1], 10);
        if (num > maxId) {
          maxId = num;
        }
      }
    });
    id = maxId + 1;

    const labelCounters: Record<string, number> = {};
    savedNodes.forEach((node) => {
      const match = node.data.label.match(/^(.*)_(\\d+)$/);
      if (match) {
        const [, kind, numStr] = match;
        const num = parseInt(numStr, 10);
        if (num > (labelCounters[kind] || 0)) {
          labelCounters[kind] = num;
        }
      }
    });

    return {
      nodes: savedNodes,
      edges: savedEdges,
      mode: savedMode,
      name: savedName,
      description: savedDescription,
      labelCounters,
    };
  } catch (error) {
    hooksLogger.warn('Could not load pipeline from local storage:', error);
    return null;
  }
}

// Drafts saved before mode existed fall back to inferring it from the
// node kinds; the inference re-runs as schemas stream in asynchronously.
function useDraftMode(draft: PipelineDraft | null, nodeDefinitions: NodeDefinition[]) {
  const [modeOverride, setMode] = useState<EngineMode | null>(draft?.mode ?? null);
  const inferredMode = useMemo<EngineMode>(() => {
    if (!draft) return 'dynamic';
    const hasOneshotNodes = draft.nodes.some((node) => {
      const nodeDef = nodeDefinitions.find((def) => def.kind === node.data.kind);
      return nodeDef?.categories.includes('oneshot');
    });
    return hasOneshotNodes ? 'oneshot' : 'dynamic';
  }, [draft, nodeDefinitions]);
  return [modeOverride ?? inferredMode, setMode] as const;
}

export const usePipeline = () => {
  const [draft] = useState(loadDraftFromStorage);
  const nodeDefinitions = useSchemaStore((s) => s.nodeDefinitions);
  const [yamlString, setYamlString] = useState<string>(
    '# Add nodes to the canvas to see YAML output'
  );
  const [mode, setMode] = useDraftMode(draft, nodeDefinitions);
  const [yamlError, setYamlError] = useState<string>('');
  const [pipelineName, setPipelineName] = useState<string>(draft?.name ?? ''),
    [pipelineDescription, setPipelineDescription] = useState<string>(draft?.description ?? '');
  const labelCountersRef = useRef<Record<string, number>>(draft?.labelCounters ?? {});
  const toast = useToast();

  const updateSourceRef = useRef<'canvas' | 'yaml' | null>(null);
  const prevNodesRef = useRef<Node<EditorNodeData>[]>([]),
    prevEdgesRef = useRef<Edge[]>([]),
    prevModeRef = useRef<EngineMode>(mode);
  const yamlDebounceTimerRef = useRef<NodeJS.Timeout | null>(null);
  const labelValidationTimerRef = useRef<NodeJS.Timeout | null>(null);
  const labelValidationTokenRef = useRef(0);

  // Stable forwarder: hydrated draft nodes capture the label handler in
  // their data before the real implementation (which needs setNodes from
  // useNodesState) can exist.
  // regenerateYamlRef holds the latest regenerateYamlFromCanvas without
  // adding it as a dependency of handleParamChange (which would cause
  // identity churn).
  const labelChangeImplRef = useRef<(nodeId: string, newLabel: string) => void>(() => {}),
    regenerateYamlRef = useRef<(snapshot?: YamlSnapshot) => void>(() => {});
  const handleLabelChange = useCallback((nodeId: string, newLabel: string) => {
    labelChangeImplRef.current(nodeId, newLabel);
  }, []);

  const handleParamChange = useCallback((nodeId: string, paramName: string, value: unknown) => {
    // Dot-notation paths (e.g. "properties.score") need to be stored as
    // nested objects so readByPath can find them.  Flat keys use the
    // simple writeNodeParam helper.
    dispatchParamUpdate(nodeId, paramName, value, writeNodeParam, (nid, config) => {
      // writeNodeParams handles the deep-merge internally.
      writeNodeParams(nid, config);
    });
    // Keep the YAML editor in sync with param changes made via the canvas
    // (e.g. compositor layer drag / slider). The guard prevents a feedback
    // loop when YAML editing triggers parseYamlToPipeline which stores the
    // onParamChange callback inside node data but never calls it inline.
    // We defer via queueMicrotask to avoid calling setState (setYamlString)
    // while React is still rendering the component that triggered this change.
    if (updateSourceRef.current !== 'yaml') {
      queueMicrotask(() => regenerateYamlRef.current());
    }
  }, []);

  const [initialDraftNodes] = useState(
    () =>
      (draft?.nodes ?? []).map((node) => ({
        ...node,
        data: {
          ...(node.data as Record<string, unknown>),
          onParamChange: handleParamChange,
          onLabelChange: handleLabelChange,
        },
      })) as unknown as Node<EditorNodeData>[]
  );

  const [nodes, setNodes, onNodesChange] = useNodesState<Node<EditorNodeData>>(initialDraftNodes);
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>(draft?.edges ?? []);

  const labelChangeImpl = useCallback(
    (nodeId: string, newLabel: string) => {
      setNodes((nds) => {
        return nds.map((node) => {
          if (node.id === nodeId) {
            return { ...node, data: { ...node.data, label: newLabel } };
          }
          return node;
        });
      });

      if (labelValidationTimerRef.current) {
        clearTimeout(labelValidationTimerRef.current);
      }

      labelValidationTokenRef.current += 1;
      const currentToken = labelValidationTokenRef.current;

      labelValidationTimerRef.current = setTimeout(() => {
        if (currentToken !== labelValidationTokenRef.current) {
          return;
        }

        setNodes((nds) => {
          const currentNode = nds.find((n) => n.id === nodeId);
          if (!currentNode) return nds;

          if (currentToken !== labelValidationTokenRef.current) {
            return nds;
          }

          if (currentNode.data.label !== newLabel) {
            return nds;
          }

          const isDuplicate = nds.some((n) => n.id !== nodeId && n.data.label === newLabel);
          if (isDuplicate) {
            setTimeout(() => {
              toast.error(
                `Node name "${newLabel}" is already in use. Please choose a unique name.`
              );
            }, 0);
          }

          return nds;
        });
      }, 500);
    },
    [setNodes, toast]
  );

  useEffect(() => {
    labelChangeImplRef.current = labelChangeImpl;
  }, [labelChangeImpl]);

  const regenerateYamlFromCanvas = useCallback(
    (snapshot?: YamlSnapshot) => {
      const nodesForYaml = snapshot?.nodes ?? nodes;
      const edgesForYaml = snapshot?.edges ?? edges;
      const modeForYaml = snapshot?.mode ?? mode;

      if (nodesForYaml.length === 0) {
        setYamlString('# Add nodes to the canvas to see YAML output');
        setYamlError('');
        return;
      }

      setYamlString(
        dump(buildPipelineForYaml(nodesForYaml, edgesForYaml, modeForYaml), { skipInvalid: true })
      );
      setYamlError('');
    },
    [nodes, edges, mode]
  );

  // Keep the ref in sync so handleParamChange always calls the latest version.
  useEffect(() => {
    regenerateYamlRef.current = regenerateYamlFromCanvas;
  }, [regenerateYamlFromCanvas]);

  const handleExportYaml = () => {
    if (nodes.length === 0) return;

    const yamlToExport = dump(
      buildPipelineForYaml(nodes, edges, mode, { includeUiPositions: true }),
      { skipInvalid: true }
    );
    const blob = new Blob([yamlToExport], { type: 'application/x-yaml' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = 'pipeline.yaml';
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
  };

  const handleImportYaml = (yamlContent: string, description = '', name = '') => {
    const result = parseYamlToPipeline(
      yamlContent,
      nodeDefinitions,
      handleParamChange,
      handleLabelChange,
      getId,
      () => {
        id = 1;
        labelCountersRef.current = {};
      }
    );

    if (result.error) {
      toast.error(`Failed to parse YAML: ${result.error}`);
      hooksLogger.error('YAML import error:', result.error);
      return;
    }

    updateSourceRef.current = 'yaml';

    result.nodes.forEach((node) => {
      clearNodeParams(node.id);
    });

    setNodes(result.nodes);
    setEdges(result.edges);
    setMode(result.mode);
    setYamlError('');
    setPipelineName(name);
    setPipelineDescription(description);
    setYamlString(yamlContent);
    toast.success('Pipeline imported successfully!');

    setTimeout(() => {
      updateSourceRef.current = null;
    }, 100);
  };

  const handleYamlChange = useCallback(
    (newYaml: string) => {
      setYamlString(newYaml);

      if (yamlDebounceTimerRef.current) {
        clearTimeout(yamlDebounceTimerRef.current);
      }

      yamlDebounceTimerRef.current = setTimeout(() => {
        const result = parseYamlToPipeline(
          newYaml,
          nodeDefinitions,
          handleParamChange,
          handleLabelChange,
          getId,
          () => {
            id = 1;
            labelCountersRef.current = {};
          }
        );

        if (result.error) {
          setYamlError(result.error);
          hooksLogger.error('YAML parsing error:', result.error);
          return;
        }

        updateSourceRef.current = 'yaml';

        result.nodes.forEach((node) => {
          clearNodeParams(node.id);
        });

        setNodes(result.nodes);
        setEdges(result.edges);
        setMode(result.mode);
        setYamlError('');

        setTimeout(() => {
          updateSourceRef.current = null;
        }, 100);
      }, 500);
    },
    [nodeDefinitions, handleParamChange, handleLabelChange, setNodes, setEdges, setMode]
  );

  useEffect(() => {
    const serializableNodes = nodes.map((node) => {
      // eslint-disable-next-line @typescript-eslint/no-unused-vars
      const { onParamChange, onLabelChange, ...restData } = node.data as EditorNodeData;

      const liveOverrides = defaultSessionStore.get(nodeParamsAtom(node.id));
      const mergedParams = { ...(restData.params || {}), ...(liveOverrides || {}) };

      return {
        ...node,
        data: {
          ...restData,
          params: Object.keys(mergedParams).length > 0 ? mergedParams : undefined,
        } as Record<string, unknown>,
      };
    });

    const pipeline = {
      nodes: serializableNodes,
      edges,
      mode,
      name: pipelineName,
      description: pipelineDescription,
    };
    try {
      window.localStorage.setItem(LOCAL_STORAGE_KEY, JSON.stringify(pipeline));
    } catch (error) {
      hooksLogger.warn('Could not save pipeline to local storage:', error);
    }
  }, [nodes, edges, mode, pipelineName, pipelineDescription]);

  const nextLabelForKind = useCallback((kind: string) => {
    const current = labelCountersRef.current[kind] ?? 0;
    const next = current + 1;
    labelCountersRef.current[kind] = next;
    return `${kind}_${next}`;
  }, []);

  useEffect(() => {
    if (updateSourceRef.current === 'yaml') {
      return;
    }

    const prevNodes = prevNodesRef.current;
    const prevEdges = prevEdgesRef.current;
    const prevMode = prevModeRef.current;

    if (prevNodes.length === nodes.length && nodes.length > 0) {
      const nodesStructurallyEqual = prevNodes.every((prev, i) => {
        const curr = nodes[i];
        return (
          curr &&
          prev.id === curr.id &&
          prev.data.kind === curr.data.kind &&
          prev.data.label === curr.data.label
        );
      });

      const edgesStructurallyEqual =
        prevEdges.length === edges.length &&
        prevEdges.every((prev, i) => {
          const curr = edges[i];
          const prevMode = (prev.data as { mode?: ConnectionMode } | undefined)?.mode;
          const currMode = (curr.data as { mode?: ConnectionMode } | undefined)?.mode;
          return (
            curr &&
            prev.id === curr.id &&
            prev.source === curr.source &&
            prev.target === curr.target &&
            prev.sourceHandle === curr.sourceHandle &&
            prev.targetHandle === curr.targetHandle &&
            prevMode === currMode
          );
        });

      if (nodesStructurallyEqual && edgesStructurallyEqual && prevMode === mode) {
        prevNodesRef.current = nodes;
        prevEdgesRef.current = edges;
        prevModeRef.current = mode;
        return;
      }
    }

    prevNodesRef.current = nodes;
    prevEdgesRef.current = edges;
    prevModeRef.current = mode;

    // Defer so the compiler-visible effect body never calls setState
    // directly; regenerateYamlFromCanvas handles the empty-canvas case.
    queueMicrotask(() => regenerateYamlRef.current({ nodes, edges, mode }));
  }, [nodes, edges, mode]);

  return {
    nodes,
    setNodes,
    onNodesChange,
    edges,
    setEdges,
    onEdgesChange,
    nodeDefinitions,
    yamlString,
    yamlError,
    isLoading: false,
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
  };
};
