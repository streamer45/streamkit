// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Shared utility for parsing YAML pipeline configurations into ReactFlow nodes and edges.
 * This eliminates duplication between handleImportYaml and handleYamlChange in usePipeline.
 */

import type { Node, Edge, XYPosition } from '@xyflow/react';
import { load } from 'js-yaml';

import type { PacketType, NodeDefinition } from '@/types/types';

import { topoLevelsFromEdges, verticalLayout } from './dag';
import {
  DEFAULT_NODE_WIDTH,
  DEFAULT_NODE_HEIGHT,
  DEFAULT_HORIZONTAL_GAP,
  DEFAULT_VERTICAL_GAP,
  ESTIMATED_HEIGHT_BY_KIND,
} from './layoutConstants';
import { canConnect, formatPacketType, resolveOutputType } from './packetTypes';

export type EngineMode = 'oneshot' | 'dynamic';

type ConnectionMode = 'reliable' | 'best_effort';

type EditorNodeData = {
  label: string;
  kind: string;
  params?: Record<string, unknown>;
  ui?: { position?: { x: number; y: number } };
  paramSchema?: unknown;
  inputs?: unknown;
  outputs?: unknown;
  definition?: { bidirectional?: boolean };
  nodeDefinition?: NodeDefinition;
  onParamChange?: (nodeId: string, paramName: string, value: unknown) => void;
  onLabelChange?: (nodeId: string, newLabel: string) => void;
};

type NeedsDependency = string | { node: string; mode?: ConnectionMode };

type ImportedNodeConfig = {
  kind: string;
  params?: Record<string, unknown>;
  ui?: { position?: XYPosition };
  needs?: NeedsDependency | NeedsDependency[];
};

export interface ParsedPipeline {
  nodes: Node<EditorNodeData>[];
  edges: Edge[];
  mode: EngineMode;
  error?: string;
}

type ParsedYaml = {
  nodes?: Record<string, ImportedNodeConfig>;
  steps?: Array<{ kind: string; params?: Record<string, unknown> }>;
  name?: string;
  description?: string;
  mode?: string;
};

/**
 * Validates the mode field in parsed YAML
 */
function validateMode(mode: string | undefined): string | null {
  if (!mode) return null;
  if (mode !== 'dynamic' && mode !== 'oneshot') {
    return `Invalid mode: "${mode}". Must be either "dynamic" or "oneshot".`;
  }
  return null;
}

/**
 * Detects the execution mode from parsed YAML
 */
function detectMode(parsed: ParsedYaml): EngineMode {
  if (parsed.mode) {
    return parsed.mode as EngineMode;
  }

  // Auto-detect from node types
  const allNodeKinds = [
    ...(parsed.steps?.map((s) => s.kind) || []),
    ...(parsed.nodes ? Object.values(parsed.nodes).map((n) => n.kind) : []),
  ];

  if (
    allNodeKinds.includes('streamkit::http_input') ||
    allNodeKinds.includes('streamkit::http_output')
  ) {
    return 'oneshot';
  }

  return 'dynamic';
}

/**
 * Converts steps format to nodes format
 */
function convertStepsToNodes(
  steps: Array<{ kind: string; params?: Record<string, unknown> }>
): Record<string, ImportedNodeConfig> {
  const nodes: Record<string, ImportedNodeConfig> = {};

  steps.forEach((step, index) => {
    const label = `step_${index}`;
    const config: ImportedNodeConfig = {
      kind: step.kind,
      params: step.params,
    };
    // Add dependency on previous step (except for first step)
    if (index > 0) {
      config.needs = `step_${index - 1}`;
    }
    nodes[label] = config;
  });

  return nodes;
}

/**
 * Normalizes parsed YAML into nodes format
 */
function normalizeParsedYaml(
  parsed: ParsedYaml
): { nodes: Record<string, ImportedNodeConfig> } | null {
  if (parsed.steps && Array.isArray(parsed.steps)) {
    return { nodes: convertStepsToNodes(parsed.steps) };
  }

  if (parsed.nodes && typeof parsed.nodes === 'object') {
    return { nodes: parsed.nodes };
  }

  return null;
}

/**
 * Expands dynamic input pins based on needs
 */
function expandDynamicInputs(nodeDef: NodeDefinition, config: ImportedNodeConfig): unknown[] {
  const needs = config.needs ? (Array.isArray(config.needs) ? config.needs : [config.needs]) : [];
  const nodeInputs = nodeDef.inputs || [];

  // If the node has a Dynamic cardinality input pin and multiple needs, expand the pins
  const dynamicPin = nodeInputs.find(
    (pin) => typeof pin.cardinality === 'object' && 'Dynamic' in pin.cardinality
  );

  if (dynamicPin && needs.length > 0) {
    // Replace the dynamic template pin with actual pins for each need
    return needs.map((_, index) => ({
      name: `in_${index}`,
      accepts_types: dynamicPin.accepts_types,
      cardinality: 'One' as const,
    }));
  }

  return nodeInputs;
}

/**
 * Creates ReactFlow nodes from pipeline nodes
 */
function createNodesFromPipeline(
  pipelineNodes: Record<string, ImportedNodeConfig>,
  nodeDefinitions: NodeDefinition[],
  getId: () => string,
  handleParamChange: (nodeId: string, paramName: string, value: unknown) => void,
  handleLabelChange: (nodeId: string, newLabel: string) => void,
  labelToIdMap: Map<string, string>,
  nodeByLabel: Map<string, Node<EditorNodeData>>,
  newNodes: Node<EditorNodeData>[]
): void {
  (Object.entries(pipelineNodes) as Array<[string, ImportedNodeConfig]>).forEach(
    ([label, config]) => {
      const nodeDef = nodeDefinitions.find((def) => def.kind === config.kind);
      if (!nodeDef) {
        throw new Error(`Unknown node kind "${config.kind}" for node "${label}".`);
      }
      const newId = getId();
      labelToIdMap.set(label, newId);

      const nodeInputs = expandDynamicInputs(nodeDef, config);

      const newNode: Node<EditorNodeData> = {
        id: newId,
        type: (config.kind === 'audio::gain' ? 'audioGain' : 'configurable') as string,
        dragHandle: '.drag-handle',
        position: (config.ui?.position as XYPosition) || {
          x: Math.random() * 800,
          y: Math.random() * 400,
        },
        data: {
          label,
          kind: config.kind as string,
          params: (config.params as Record<string, unknown>) || {},
          paramSchema: nodeDef.param_schema,
          inputs: nodeInputs,
          outputs: nodeDef.outputs || [],
          definition: { bidirectional: nodeDef.bidirectional },
          nodeDefinition: nodeDef,
          onParamChange: handleParamChange,
          onLabelChange: handleLabelChange,
          // No sessionId - prevents LIVE badge in design view
        },
        origin: [0.5, 0] as [number, number],
      };
      newNodes.push(newNode);
      nodeByLabel.set(label, newNode);
    }
  );
}

/**
 * Validates connection compatibility between source and target nodes
 */
function validateConnectionCompatibility(
  sourceNode: Node<EditorNodeData>,
  targetNode: Node<EditorNodeData>,
  needsIndex: number,
  sourceLabel: string,
  targetLabel: string,
  nodes: Node<EditorNodeData>[],
  edges: Edge[]
): void {
  const sourceOutputs = (sourceNode.data.outputs || []) as Array<{
    name: string;
    produces_type: PacketType;
  }>;
  const targetInputs = (targetNode.data.inputs || []) as Array<{
    name: string;
    accepts_types: PacketType[];
  }>;

  // Get default output pin (first output)
  const sourceOutput = sourceOutputs[0];
  // For nodes with multiple needs, use the input pin corresponding to the needs index
  const targetInput = targetInputs[needsIndex] || targetInputs[0];

  if (!sourceOutput || !targetInput) return;

  // Resolve the output type (handles Passthrough inference and param-dependent nodes like resampler)
  const resolvedSourceType = resolveOutputType(
    sourceNode,
    sourceOutput.name,
    nodes as unknown as Node[],
    edges
  );

  // Use the existing canConnect utility for proper type validation
  const isCompatible = canConnect(resolvedSourceType, targetInput.accepts_types);

  if (!isCompatible) {
    const sourceTypeStr = formatPacketType(resolvedSourceType);
    const targetTypesStr = targetInput.accepts_types.map((t) => formatPacketType(t)).join(' or ');
    throw new Error(
      `Incompatible connection: "${sourceLabel}" produces ${sourceTypeStr} but "${targetLabel}" accepts ${targetTypesStr}.`
    );
  }
}

/**
 * Creates an edge between two nodes
 */
function createEdgeForConnection(
  sourceNode: Node<EditorNodeData>,
  targetNode: Node<EditorNodeData>,
  sourceId: string,
  targetId: string,
  needsIndex: number,
  mode: ConnectionMode | undefined,
  nodeByLabel: Map<string, Node<EditorNodeData>>,
  newEdges: Edge[]
): void {
  const sourceOutputs = (sourceNode.data.outputs || []) as Array<{
    name: string;
    produces_type: PacketType;
  }>;
  const targetInputs = (targetNode.data.inputs || []) as Array<{
    name: string;
    accepts_types: PacketType[];
  }>;

  const sourceOutput = sourceOutputs[0];
  const targetInput = targetInputs[needsIndex] || targetInputs[0];

  if (!sourceOutput || !targetInput) return;

  // Get pin names
  const sourceHandleName = sourceOutput.name;
  const targetHandleName = targetInput.name;

  // Resolve the output type for this edge (handles Passthrough inference)
  const resolvedType = resolveOutputType(
    sourceNode,
    sourceHandleName,
    Array.from(nodeByLabel.values()),
    newEdges
  );

  // Ensure unique edge ids in case of multiple connections between the same nodes
  newEdges.push({
    id: `${sourceId}-${targetId}-${newEdges.length}`,
    source: sourceId,
    target: targetId,
    sourceHandle: sourceHandleName,
    targetHandle: targetHandleName,
    data: {
      resolvedType,
      ...(mode === 'best_effort' ? { mode } : {}),
    },
  });
}

/**
 * Creates edges from pipeline connections
 */
function createEdgesFromPipeline(
  pipelineNodes: Record<string, ImportedNodeConfig>,
  labelToIdMap: Map<string, string>,
  nodeByLabel: Map<string, Node<EditorNodeData>>,
  newEdges: Edge[]
): void {
  (Object.entries(pipelineNodes) as Array<[string, ImportedNodeConfig]>).forEach(
    ([label, config]) => {
      if (!config.needs) return;

      const targetId = labelToIdMap.get(label);
      const targetNode = nodeByLabel.get(label);
      if (!targetId || !targetNode) return;

      const needs: NeedsDependency[] = Array.isArray(config.needs) ? config.needs : [config.needs];

      needs.forEach((dep: NeedsDependency, needsIndex: number) => {
        const sourceLabel = typeof dep === 'string' ? dep : dep.node;
        const mode: ConnectionMode | undefined = typeof dep === 'string' ? undefined : dep.mode;

        const sourceId = labelToIdMap.get(sourceLabel);

        // Validate that the referenced node exists
        if (!sourceId) {
          throw new Error(
            `Node "${label}" references non-existent node "${sourceLabel}" in needs.`
          );
        }

        const sourceNode = nodeByLabel.get(sourceLabel);
        if (!sourceNode) return;

        // Validate connection compatibility
        validateConnectionCompatibility(
          sourceNode,
          targetNode,
          needsIndex,
          sourceLabel,
          label,
          Array.from(nodeByLabel.values()),
          newEdges
        );

        // Create the edge
        createEdgeForConnection(
          sourceNode,
          targetNode,
          sourceId,
          targetId,
          needsIndex,
          mode,
          nodeByLabel,
          newEdges
        );
      });
    }
  );
}

/**
 * Applies automatic layout to nodes using DAG utilities
 */
function applyAutomaticLayout(
  newNodes: Node<EditorNodeData>[],
  newEdges: Edge[]
): Node<EditorNodeData>[] {
  const nodeWidth = DEFAULT_NODE_WIDTH;
  const nodeHeight = DEFAULT_NODE_HEIGHT;
  const hGap = DEFAULT_HORIZONTAL_GAP;
  const vGap = DEFAULT_VERTICAL_GAP;

  const nodeIds = newNodes.map((n) => n.id);
  const { levels, sortedLevels } = topoLevelsFromEdges(
    nodeIds,
    newEdges.map((e) => ({ source: e.source, target: e.target }))
  );

  // Provide estimated per-node heights so vertical spacing between levels ignores node heights
  const kindById: Record<string, string> = {};
  newNodes.forEach((n) => {
    kindById[n.id] = n.data.kind as string;
  });
  const perNodeHeights: Record<string, number> = {};
  nodeIds.forEach((id) => {
    const kind = kindById[id];
    perNodeHeights[id] = ESTIMATED_HEIGHT_BY_KIND[kind] ?? DEFAULT_NODE_HEIGHT;
  });

  const positions = verticalLayout(levels, sortedLevels, {
    nodeWidth,
    nodeHeight,
    hGap,
    vGap,
    heights: perNodeHeights,
    edges: newEdges.map((e) => ({ source: e.source, target: e.target })),
  });

  return newNodes.map((n) => ({
    ...n,
    position: (positions[n.id] as XYPosition | undefined) ?? (n.position as XYPosition),
  })) as Node<EditorNodeData>[];
}

/**
 * Parse YAML content into ReactFlow nodes and edges with automatic layout.
 *
 * @param yamlContent - The YAML string to parse
 * @param nodeDefinitions - Available node type definitions from the schema store
 * @param handleParamChange - Callback for parameter changes
 * @param handleLabelChange - Callback for label changes
 * @param getId - Function to generate unique node IDs
 * @param resetCounters - Function to reset label counters
 * @returns Parsed pipeline with nodes, edges, and mode, or an error message
 */
export const parseYamlToPipeline = (
  yamlContent: string,
  nodeDefinitions: NodeDefinition[],
  handleParamChange: (nodeId: string, paramName: string, value: unknown) => void,
  handleLabelChange: (nodeId: string, newLabel: string) => void,
  getId: () => string,
  resetCounters: () => void
): ParsedPipeline => {
  try {
    const parsed = load(yamlContent) as ParsedYaml;

    if (!parsed || typeof parsed !== 'object') {
      return { nodes: [], edges: [], mode: 'dynamic', error: 'Invalid YAML: Must be an object' };
    }

    // Validate mode
    const modeError = validateMode(parsed.mode);
    if (modeError) {
      return { nodes: [], edges: [], mode: 'dynamic', error: modeError };
    }

    // Detect execution mode
    const detectedMode = detectMode(parsed);

    // Normalize to nodes format
    const pipeline = normalizeParsedYaml(parsed);
    if (!pipeline) {
      return {
        nodes: [],
        edges: [],
        mode: detectedMode,
        error: 'Invalid YAML: Must contain either "steps" array or "nodes" object',
      };
    }

    const newNodes: Node<EditorNodeData>[] = [];
    const newEdges: Edge[] = [];
    const labelToIdMap = new Map<string, string>();
    const nodeByLabel = new Map<string, Node<EditorNodeData>>();

    // Reset counters
    resetCounters();

    // First pass: create nodes and expand dynamic input pins based on needs
    createNodesFromPipeline(
      pipeline.nodes,
      nodeDefinitions,
      getId,
      handleParamChange,
      handleLabelChange,
      labelToIdMap,
      nodeByLabel,
      newNodes
    );

    // Second pass: validate and create edges
    createEdgesFromPipeline(pipeline.nodes, labelToIdMap, nodeByLabel, newEdges);

    // Auto-layout imported pipelines top-to-bottom using shared DAG utilities
    const laidOutNodes = applyAutomaticLayout(newNodes, newEdges);

    return {
      nodes: laidOutNodes,
      edges: newEdges,
      mode: detectedMode,
    };
  } catch (e: unknown) {
    const message = e instanceof Error ? e.message : String(e);
    return {
      nodes: [],
      edges: [],
      mode: 'dynamic',
      error: message,
    };
  }
};

export { injectFileReadNode } from './yamlFileReadInjection';
