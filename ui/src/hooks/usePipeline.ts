// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useNodesState, useEdgesState, type Node, type Edge } from '@xyflow/react';
import { dump } from 'js-yaml';
import { useState, useEffect, useRef, useCallback } from 'react';

import { useToast } from '@/context/ToastContext';
import { useNodeParamsStore } from '@/stores/nodeParamsStore';
import { useSchemaStore } from '@/stores/schemaStore';
import { topoOrderFromEdges } from '@/utils/dag';
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
  const [pipelineName, setPipelineName] = useState<string>('');
  const [pipelineDescription, setPipelineDescription] = useState<string>('');
  const labelCountersRef = useRef<Record<string, number>>({});
  const toast = useToast();

  // Track update source to prevent circular updates
  const updateSourceRef = useRef<'canvas' | 'yaml' | null>(null);
  // Track previous nodes for structural change detection
  const prevNodesRef = useRef<Node<EditorNodeData>[]>([]);
  const yamlDebounceTimerRef = useRef<NodeJS.Timeout | null>(null);
  const labelValidationTimerRef = useRef<NodeJS.Timeout | null>(null);
  // Track the validation token to invalidate stale validations
  const labelValidationTokenRef = useRef(0);

  const handleLabelChange = useCallback(
    (nodeId: string, newLabel: string) => {
      // Update the label immediately for responsive editing
      setNodes((nds) => {
        return nds.map((node) => {
          if (node.id === nodeId) {
            return { ...node, data: { ...node.data, label: newLabel } };
          }
          return node;
        });
      });

      // Clear any existing validation timer
      if (labelValidationTimerRef.current) {
        clearTimeout(labelValidationTimerRef.current);
      }

      // Increment token to invalidate any pending validation callbacks
      labelValidationTokenRef.current += 1;
      const currentToken = labelValidationTokenRef.current;

      // Debounce the duplicate validation to avoid interrupting typing
      labelValidationTimerRef.current = setTimeout(() => {
        // Check if this validation is still valid (not superseded by newer input)
        if (currentToken !== labelValidationTokenRef.current) {
          return;
        }

        // Use a separate function call to access nodes state and validate
        setNodes((nds) => {
          // Find the current label for this node
          const currentNode = nds.find((n) => n.id === nodeId);
          if (!currentNode) return nds;

          // Double-check token hasn't changed while we were waiting
          if (currentToken !== labelValidationTokenRef.current) {
            return nds;
          }

          // Only validate if the current label still matches what we scheduled
          if (currentNode.data.label !== newLabel) {
            return nds;
          }

          const isDuplicate = nds.some((n) => n.id !== nodeId && n.data.label === newLabel);

          // Schedule toast for next tick to avoid calling setState during render
          if (isDuplicate) {
            setTimeout(() => {
              toast.error(
                `Node name "${newLabel}" is already in use. Please choose a unique name.`
              );
            }, 0);
          }

          return nds;
        });
      }, 500); // 500ms debounce
    },
    [setNodes, toast]
  );

  const setParam = useNodeParamsStore((s) => s.setParam);
  const resetNode = useNodeParamsStore((s) => s.resetNode);

  const handleParamChange = useCallback(
    (nodeId: string, paramName: string, value: unknown) => {
      // Write live param changes to a lightweight store instead of mutating the nodes array.
      // This keeps ReactFlow props stable and avoids re-rendering the canvas on every slider tick.
      setParam(nodeId, paramName, value);
    },
    [setParam]
  );

  const handleExportYaml = () => {
    if (nodes.length === 0) return;

    const idToLabelMap = new Map(nodes.map((n) => [n.id, n.data.label]));
    const idToNode = new Map(nodes.map((n) => [n.id, n]));
    const pipeline: { mode: EngineMode; nodes: Record<string, unknown> } = { mode, nodes: {} };

    // Compute topological order of nodes based on edges (sources first)
    const nodeIds = nodes.map((n) => n.id);
    const orderedIds = topoOrderFromEdges(
      nodeIds,
      edges.map((e) => ({ source: e.source, target: e.target }))
    );

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

      const nodeConfig: Record<string, unknown> = {
        kind: node.data.kind,
        ui: {
          position: {
            x: Math.round(node.position.x),
            y: Math.round(node.position.y),
          },
        },
      };

      // Merge any live overrides from the params store so exports reflect current UI values
      const overrides = (
        useNodeParamsStore.getState().paramsById as Record<string, Record<string, unknown>>
      )[node.id as string];
      const mergedParams = { ...(node.data.params || {}), ...(overrides || {}) };
      if (Object.keys(mergedParams).length > 0) {
        nodeConfig['params'] = mergedParams;
      }

      if (needs.length === 1) {
        (nodeConfig as Record<string, unknown>)['needs'] = needs[0];
      } else if (needs.length > 1) {
        (nodeConfig as Record<string, unknown>)['needs'] = needs;
      }

      pipeline.nodes[node.data.label] = nodeConfig;
    });

    const yamlToExport = dump(pipeline, { skipInvalid: true });
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

    // Clear stale parameter overrides for all nodes so sliders can read fresh values from YAML
    result.nodes.forEach((node) => {
      resetNode(node.id);
    });

    setNodes(result.nodes);
    setEdges(result.edges);
    setMode(result.mode);
    setYamlError('');
    setPipelineName(name);
    setPipelineDescription(description);
    // Update YAML string immediately so it shows in the editor
    setYamlString(yamlContent);
    toast.success('Pipeline imported successfully!');

    // Reset update source after React finishes processing and the YAML regeneration effect has run
    setTimeout(() => {
      updateSourceRef.current = null;
    }, 100);
  };

  // Handle YAML changes from the editor (debounced)
  const handleYamlChange = useCallback(
    (newYaml: string) => {
      // Update the YAML string immediately for responsive editing
      setYamlString(newYaml);

      // Clear any existing timer
      if (yamlDebounceTimerRef.current) {
        clearTimeout(yamlDebounceTimerRef.current);
      }

      // Debounce the parsing to avoid rapid updates while typing
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

        // Mark that this update came from YAML to prevent circular updates
        updateSourceRef.current = 'yaml';

        // Clear stale parameter overrides for all nodes so sliders can read fresh values from YAML
        result.nodes.forEach((node) => {
          resetNode(node.id);
        });

        setNodes(result.nodes);
        setEdges(result.edges);
        setMode(result.mode);
        setYamlError('');

        // Reset update source after React finishes processing and the YAML regeneration effect has run
        // This needs to be longer than setTimeout(..., 0) to ensure the effect runs with the flag set
        setTimeout(() => {
          updateSourceRef.current = null;
        }, 100);
      }, 500); // 500ms debounce
    },
    [nodeDefinitions, handleParamChange, handleLabelChange, setNodes, setEdges, setMode, resetNode]
  );

  // Load from localStorage on initial mount
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
          // Re-initialize the global ID counter to avoid collisions
          let maxId = 0;
          savedNodes.forEach((node) => {
            const match = node.id.match(/^skitnode_(\d+)$/);
            if (match) {
              const num = parseInt(match[1], 10);
              if (num > maxId) {
                maxId = num;
              }
            }
          });
          id = maxId + 1;

          // Re-initialize label counters
          const newCounters: Record<string, number> = {};
          savedNodes.forEach((node) => {
            const match = node.data.label.match(/^(.*)_(\d+)$/);
            if (match) {
              const [, kind, numStr] = match;
              const num = parseInt(numStr, 10);
              if (num > (newCounters[kind] || 0)) {
                newCounters[kind] = num;
              }
            }
          });
          labelCountersRef.current = newCounters;

          // Re-hydrate nodes with callback functions
          const hydratedNodes = savedNodes.map((node) => ({
            ...node,
            data: {
              ...(node.data as Record<string, unknown>),
              onParamChange: handleParamChange,
              onLabelChange: handleLabelChange,
              // No sessionId - prevents LIVE badge in design view
            },
          })) as unknown as Node<EditorNodeData>[];

          setNodes(hydratedNodes);
          setEdges(savedEdges);

          // Restore name and description if saved
          if (savedName) {
            setPipelineName(savedName);
          }
          if (savedDescription) {
            setPipelineDescription(savedDescription);
          }

          // Auto-detect mode from loaded nodes if not explicitly saved
          if (savedMode) {
            setMode(savedMode);
          } else {
            // Auto-detect from node types
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
  }, [handleParamChange, handleLabelChange, setNodes, setEdges, setMode, nodeDefinitions]); // Dependencies are stable; runs effectively once

  // Save to localStorage when nodes, edges, or mode change
  useEffect(() => {
    if (isLoading) {
      return; // Don't save on initial render before hydration is complete
    }

    const serializableNodes = nodes.map((node) => {
      // eslint-disable-next-line @typescript-eslint/no-unused-vars
      const { onParamChange, onLabelChange, ...restData } = node.data as EditorNodeData;

      // Merge live parameter overrides from nodeParamsStore into node.data.params
      // so they persist across page reloads
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

  // Track per-kind counters to generate default human-readable labels
  const nextLabelForKind = useCallback((kind: string) => {
    const current = labelCountersRef.current[kind] ?? 0;
    const next = current + 1;
    labelCountersRef.current[kind] = next;
    return `${kind}_${next}`;
  }, []);

  useEffect(() => {
    // Don't regenerate YAML if the update came from YAML editor
    if (updateSourceRef.current === 'yaml') {
      return;
    }

    // Check if only non-structural changes occurred (e.g., position changes)
    // This prevents YAML regeneration when dragging nodes around the canvas
    const prevNodes = prevNodesRef.current;
    if (prevNodes.length === nodes.length && nodes.length > 0) {
      const structurallyEqual = prevNodes.every((prev, i) => {
        const curr = nodes[i];
        return (
          curr &&
          prev.id === curr.id &&
          prev.data.kind === curr.data.kind &&
          prev.data.label === curr.data.label
        );
      });

      if (structurallyEqual) {
        // Only positions changed, skip YAML regeneration
        prevNodesRef.current = nodes;
        return;
      }
    }

    // Update ref with current nodes for next comparison
    prevNodesRef.current = nodes;

    // Generate YAML from nodes and edges
    if (nodes.length === 0) {
      setYamlString('# Add nodes to the canvas to see YAML output');
      setYamlError('');
      return;
    }

    const idToLabelMap = new Map(nodes.map((n) => [n.id, n.data.label]));
    const idToNode = new Map(nodes.map((n) => [n.id, n]));
    const pipeline: { mode: EngineMode; nodes: Record<string, unknown> } = { mode, nodes: {} };

    // Compute topological order of nodes based on edges (sources first)
    const nodeIds = nodes.map((n) => n.id);
    const orderedIds = topoOrderFromEdges(
      nodeIds,
      edges.map((e) => ({ source: e.source, target: e.target }))
    );

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

      const nodeConfig: Record<string, unknown> = {
        kind: node.data.kind,
      };

      // Merge live overrides from the params store so the YAML reflects real-time UI values
      const overrides = useNodeParamsStore.getState().paramsById[node.id];
      const mergedParams = { ...(node.data.params || {}), ...(overrides || {}) };
      if (Object.keys(mergedParams).length > 0) {
        (nodeConfig as Record<string, unknown>)['params'] = mergedParams;
      }

      if (needs.length === 1) {
        (nodeConfig as Record<string, unknown>)['needs'] = needs[0];
      } else if (needs.length > 1) {
        (nodeConfig as Record<string, unknown>)['needs'] = needs;
      }

      pipeline.nodes[node.data.label] = nodeConfig;
    });

    setYamlString(dump(pipeline, { skipInvalid: true }));
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
    getId,
  };
};
