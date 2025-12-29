// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useNodesState, useEdgesState, type Node, type Edge } from '@xyflow/react';
import { dump } from 'js-yaml';
import { useState, useEffect, useRef, useCallback } from 'react';

import { useToast } from '@/context/ToastContext';
import { useNodeParamsStore } from '@/stores/nodeParamsStore';
import { useSchemaStore } from '@/stores/schemaStore';
import { hooksLogger } from '@/utils/logger';
import { parseYamlToPipeline, type EngineMode } from '@/utils/yamlPipeline';

const LOCAL_STORAGE_KEY = 'sk-pipeline-draft';

let id = 1;
const getId = () => `skitnode_${id++}`;

type EditorNodeData = {
  label: string;
  kind: string;
  params?: Record<string, unknown>;
  ui?: { position?: { x: number; y: number } };
  paramSchema?: unknown;
  inputs?: unknown;
  outputs?: unknown;
  definition?: { bidirectional?: boolean };
  onParamChange?: (nodeId: string, paramName: string, value: unknown) => void;
  onLabelChange?: (nodeId: string, newLabel: string) => void;
};

type ConnectionMode = 'reliable' | 'best_effort';
type NeedsDependency = string | { node: string; mode?: ConnectionMode };

function orderNodeIdsTopDown(
  nodes: Array<Node<EditorNodeData>>,
  edges: Array<Edge>
): Array<string> {
  const nodeIds = nodes.map((n) => n.id);
  const posById = new Map(nodeIds.map((id) => [id, { x: 0, y: 0 }]));
  nodes.forEach((n) => posById.set(n.id, { x: n.position.x, y: n.position.y }));

  const inDegree: Record<string, number> = {};
  const outgoing: Record<string, string[]> = {};
  nodeIds.forEach((nodeId) => {
    inDegree[nodeId] = 0;
    outgoing[nodeId] = [];
  });

  edges.forEach((e) => {
    if (!(e.source in outgoing) || !(e.target in inDegree)) return;
    outgoing[e.source].push(e.target);
    inDegree[e.target] += 1;
  });

  const compare = (a: string, b: string) => {
    const pa = posById.get(a) ?? { x: 0, y: 0 };
    const pb = posById.get(b) ?? { x: 0, y: 0 };
    if (pa.y !== pb.y) return pa.y - pb.y;
    if (pa.x !== pb.x) return pa.x - pb.x;
    return a.localeCompare(b);
  };

  const queue = nodeIds.filter((nodeId) => inDegree[nodeId] === 0).sort(compare);
  const ordered: string[] = [];
  const seen = new Set<string>();

  while (queue.length > 0) {
    const u = queue.shift() as string;
    if (seen.has(u)) continue;
    seen.add(u);
    ordered.push(u);
    for (const v of outgoing[u]) {
      inDegree[v] -= 1;
      if (inDegree[v] === 0) {
        queue.push(v);
      }
    }
    queue.sort(compare);
  }

  const remaining = nodeIds.filter((nodeId) => !seen.has(nodeId)).sort(compare);
  return [...ordered, ...remaining];
}

function buildPipelineForYaml(
  nodes: Array<Node<EditorNodeData>>,
  edges: Array<Edge>,
  mode: EngineMode,
  opts?: { includeUiPositions?: boolean }
): { mode: EngineMode; nodes: Record<string, unknown> } {
  const includeUiPositions = opts?.includeUiPositions ?? false;
  const idToLabelMap = new Map(nodes.map((n) => [n.id, n.data.label]));
  const idToNode = new Map(nodes.map((n) => [n.id, n]));
  const pipeline: { mode: EngineMode; nodes: Record<string, unknown> } = { mode, nodes: {} };

  const orderedIds = orderNodeIdsTopDown(nodes, edges);

  orderedIds.forEach((nodeId) => {
    const node = idToNode.get(nodeId);
    if (!node) return;

    const needs = edges
      .filter((e) => e.target === node.id)
      .map((e): NeedsDependency | null => {
        const label = idToLabelMap.get(e.source);
        if (!label) return null;
        const mode = (e.data as { mode?: ConnectionMode } | undefined)?.mode;
        return mode === 'best_effort' ? { node: label, mode } : label;
      })
      .filter((v): v is NeedsDependency => v !== null);

    const nodeConfig: Record<string, unknown> = { kind: node.data.kind };

    if (includeUiPositions) {
      nodeConfig['ui'] = {
        position: {
          x: Math.round(node.position.x),
          y: Math.round(node.position.y),
        },
      };
    }

    const overrides = useNodeParamsStore.getState().paramsById[node.id];
    const mergedParams = { ...(node.data.params || {}), ...(overrides || {}) };
    if (Object.keys(mergedParams).length > 0) {
      nodeConfig['params'] = mergedParams;
    }

    if (needs.length === 1) {
      nodeConfig['needs'] = needs[0];
    } else if (needs.length > 1) {
      nodeConfig['needs'] = needs;
    }

    pipeline.nodes[node.data.label] = nodeConfig;
  });

  return pipeline;
}

export const usePipeline = () => {
  const [nodes, setNodes, onNodesChange] = useNodesState<Node<EditorNodeData>>([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>([]);
  const nodeDefinitions = useSchemaStore((s) => s.nodeDefinitions);
  const [yamlString, setYamlString] = useState<string>(
    '# Add nodes to the canvas to see YAML output'
  );
  const [isLoading, setIsLoading] = useState(true);
  const [mode, setMode] = useState<EngineMode>('dynamic');
  const [yamlError, setYamlError] = useState<string>('');
  const [pipelineName, setPipelineName] = useState<string>(''),
    [pipelineDescription, setPipelineDescription] = useState<string>('');
  const labelCountersRef = useRef<Record<string, number>>({});
  const toast = useToast();

  const updateSourceRef = useRef<'canvas' | 'yaml' | null>(null);
  const prevNodesRef = useRef<Node<EditorNodeData>[]>([]),
    prevEdgesRef = useRef<Edge[]>([]),
    prevModeRef = useRef<EngineMode>(mode);
  const yamlDebounceTimerRef = useRef<NodeJS.Timeout | null>(null);
  const labelValidationTimerRef = useRef<NodeJS.Timeout | null>(null);
  const labelValidationTokenRef = useRef(0);

  const handleLabelChange = useCallback(
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

  const setParam = useNodeParamsStore((s) => s.setParam);
  const resetNode = useNodeParamsStore((s) => s.resetNode);

  const handleParamChange = useCallback(
    (nodeId: string, paramName: string, value: unknown) => {
      setParam(nodeId, paramName, value);
    },
    [setParam]
  );

  const regenerateYamlFromCanvas = useCallback(
    (snapshot?: {
      nodes?: Array<Node<EditorNodeData>>;
      edges?: Array<Edge>;
      mode?: EngineMode;
    }) => {
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
      resetNode(node.id);
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
          resetNode(node.id);
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
    [nodeDefinitions, handleParamChange, handleLabelChange, setNodes, setEdges, setMode, resetNode]
  );

  useEffect(() => {
    try {
      const item = window.localStorage.getItem(LOCAL_STORAGE_KEY);
      if (item) {
        const {
          nodes: savedNodes,
          edges: savedEdges,
          mode: savedMode,
          name: savedName,
          description: savedDescription,
        } = JSON.parse(item) as {
          nodes: Array<
            Node<{
              label: string;
              kind: string;
              params?: Record<string, unknown>;
              ui?: { position?: { x: number; y: number } };
            }>
          >;
          edges: Edge[];
          mode?: EngineMode;
          name?: string;
          description?: string;
        };

        if (Array.isArray(savedNodes) && Array.isArray(savedEdges)) {
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

          const newCounters: Record<string, number> = {};
          savedNodes.forEach((node) => {
            const match = node.data.label.match(/^(.*)_(\\d+)$/);
            if (match) {
              const [, kind, numStr] = match;
              const num = parseInt(numStr, 10);
              if (num > (newCounters[kind] || 0)) {
                newCounters[kind] = num;
              }
            }
          });
          labelCountersRef.current = newCounters;

          const hydratedNodes = savedNodes.map((node) => ({
            ...node,
            data: {
              ...(node.data as Record<string, unknown>),
              onParamChange: handleParamChange,
              onLabelChange: handleLabelChange,
            },
          })) as unknown as Node<EditorNodeData>[];

          setNodes(hydratedNodes);
          setEdges(savedEdges);

          if (savedName) {
            setPipelineName(savedName);
          }
          if (savedDescription) {
            setPipelineDescription(savedDescription);
          }

          if (savedMode) {
            setMode(savedMode);
          } else {
            const hasOneshotNodes = hydratedNodes.some((node) => {
              const nodeDef = nodeDefinitions.find((def) => def.kind === node.data.kind);
              return nodeDef?.categories.includes('oneshot');
            });
            setMode(hasOneshotNodes ? 'oneshot' : 'dynamic');
          }
        }
      }
    } catch (error) {
      hooksLogger.warn('Could not load pipeline from local storage:', error);
    } finally {
      setIsLoading(false);
    }
  }, [handleParamChange, handleLabelChange, setNodes, setEdges, setMode, nodeDefinitions]);

  useEffect(() => {
    if (isLoading) {
      return;
    }

    const serializableNodes = nodes.map((node) => {
      // eslint-disable-next-line @typescript-eslint/no-unused-vars
      const { onParamChange, onLabelChange, ...restData } = node.data as EditorNodeData;

      const liveOverrides = useNodeParamsStore.getState().paramsById[node.id];
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
  }, [nodes, edges, mode, pipelineName, pipelineDescription, isLoading]);

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

    if (nodes.length === 0) {
      setYamlString('# Add nodes to the canvas to see YAML output');
      setYamlError('');
      return;
    }

    setYamlString(dump(buildPipelineForYaml(nodes, edges, mode), { skipInvalid: true }));
    setYamlError('');
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
    isLoading,
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
