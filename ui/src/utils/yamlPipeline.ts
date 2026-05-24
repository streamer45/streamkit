// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

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
type NeedsValue = NeedsDependency | NeedsDependency[] | Record<string, string>;

type ParsedDependency = {
  sourceLabel: string;
  sourcePin?: string;
  mode?: ConnectionMode;
};

function parseDependency(dep: NeedsDependency): ParsedDependency {
  const mode = typeof dep === 'string' ? undefined : dep.mode;
  const raw = typeof dep === 'string' ? dep : dep.node;

  const [sourceLabel, ...rest] = raw.split('.');
  const sourcePin = rest.length > 0 ? rest.join('.') : undefined;

  return { sourceLabel, sourcePin, mode };
}

type ImportedNodeConfig = {
  kind: string;
  params?: Record<string, unknown>;
  ui?: { position?: XYPosition };
  needs?: NeedsValue;
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

function validateMode(mode: string | undefined): string | null {
  if (!mode) return null;
  if (mode !== 'dynamic' && mode !== 'oneshot') {
    return `Invalid mode: "${mode}". Must be either "dynamic" or "oneshot".`;
  }
  return null;
}

function detectMode(parsed: ParsedYaml): EngineMode {
  if (parsed.mode) {
    return parsed.mode as EngineMode;
  }

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
    if (index > 0) {
      config.needs = `step_${index - 1}`;
    }
    nodes[label] = config;
  });

  return nodes;
}

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

function expandDynamicInputs(nodeDef: NodeDefinition, config: ImportedNodeConfig): unknown[] {
  const needs = config.needs ? (Array.isArray(config.needs) ? config.needs : [config.needs]) : [];
  const nodeInputs = nodeDef.inputs || [];

  const dynamicPin = nodeInputs.find(
    (pin) => typeof pin.cardinality === 'object' && 'Dynamic' in pin.cardinality
  );

  if (dynamicPin && needs.length > 0) {
    return needs.map((_, index) => ({
      name: `in_${index}`,
      accepts_types: dynamicPin.accepts_types,
      cardinality: 'One' as const,
    }));
  }

  return nodeInputs;
}

/** Derive output pins for http_input based on params.fields/field. */
function deriveHttpInputOutputs(
  params?: Record<string, unknown>
): Array<{ name: string; produces_type: PacketType; cardinality: 'Broadcast' }> {
  const normalizeFieldName = (value: unknown): string | null => {
    if (typeof value === 'string' && value.trim()) {
      return value.trim();
    }
    if (value && typeof value === 'object' && 'name' in (value as Record<string, unknown>)) {
      const name = String((value as Record<string, unknown>).name || '').trim();
      return name || null;
    }
    return null;
  };

  const fields = params?.fields;
  const outputs: string[] = Array.isArray(fields)
    ? fields
        .map((entry) => normalizeFieldName(entry))
        .filter((name): name is string => Boolean(name))
    : [];

  if (outputs.length === 0) {
    const single = normalizeFieldName(params?.field);
    outputs.push(single ?? 'media');
  }

  const unique = Array.from(new Set(outputs));
  return unique.map((name) => ({
    name,
    produces_type: 'Binary' as PacketType,
    cardinality: 'Broadcast' as const,
  }));
}

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
      const nodeOutputs =
        config.kind === 'streamkit::http_input'
          ? deriveHttpInputOutputs(config.params)
          : nodeDef.outputs || [];

      const newNode: Node<EditorNodeData> = {
        id: newId,
        type: (config.kind === 'audio::gain'
          ? 'audioGain'
          : config.kind === 'video::compositor'
            ? 'compositor'
            : 'configurable') as string,
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
          outputs: nodeOutputs,
          definition: { bidirectional: nodeDef.bidirectional },
          nodeDefinition: nodeDef,
          onParamChange: handleParamChange,
          onLabelChange: handleLabelChange,
        },
        origin: [0.5, 0] as [number, number],
      };
      newNodes.push(newNode);
      nodeByLabel.set(label, newNode);
    }
  );
}

function validateConnectionCompatibility(
  sourceNode: Node<EditorNodeData>,
  targetNode: Node<EditorNodeData>,
  sourceHandleName: string,
  targetHandleName: string,
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

  const sourceOutput = sourceOutputs.find((o) => o.name === sourceHandleName) ?? sourceOutputs[0];
  const targetInput = targetInputs.find((i) => i.name === targetHandleName) ?? targetInputs[0];

  if (!sourceOutput) {
    throw new Error(
      `Node "${targetLabel}" references non-existent pin "${sourceHandleName}" on "${sourceLabel}".`
    );
  }
  if (!targetInput) return;

  const resolvedSourceType = resolveOutputType(
    sourceNode,
    sourceOutput.name,
    nodes as unknown as Node[],
    edges
  );

  const isCompatible = canConnect(resolvedSourceType, targetInput.accepts_types);

  if (!isCompatible) {
    const sourceTypeStr = formatPacketType(resolvedSourceType);
    const targetTypesStr = targetInput.accepts_types.map((t) => formatPacketType(t)).join(' or ');
    throw new Error(
      `Incompatible connection: "${sourceLabel}" produces ${sourceTypeStr} but "${targetLabel}" accepts ${targetTypesStr}.`
    );
  }
}

function createEdgeForConnection(
  sourceNode: Node<EditorNodeData>,
  targetNode: Node<EditorNodeData>,
  sourceId: string,
  targetId: string,
  sourceHandleName: string,
  targetHandleName: string,
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

  const sourceOutput = sourceOutputs.find((o) => o.name === sourceHandleName) ?? sourceOutputs[0];
  const targetInput = targetInputs.find((i) => i.name === targetHandleName) ?? targetInputs[0];

  if (!sourceOutput || !targetInput) return;

  const sourceHandle = sourceOutput.name;
  const targetHandle = targetInput.name;

  const resolvedType = resolveOutputType(
    sourceNode,
    sourceHandle,
    Array.from(nodeByLabel.values()),
    newEdges
  );

  newEdges.push({
    id: `${sourceId}-${targetId}-${newEdges.length}`,
    source: sourceId,
    target: targetId,
    sourceHandle: sourceHandle,
    targetHandle: targetHandle,
    data: {
      resolvedType,
      ...(mode === 'best_effort' ? { mode } : {}),
    },
  });
}

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

      const rawNeeds = config.needs;
      const isMapStyle =
        typeof rawNeeds === 'object' && !Array.isArray(rawNeeds) && !('node' in rawNeeds);

      if (isMapStyle) {
        const needsMap = rawNeeds as Record<string, string>;
        Object.entries(needsMap).forEach(([targetPin, sourceRef]) => {
          const { sourceLabel, sourcePin } = parseDependency(sourceRef);
          const srcId = labelToIdMap.get(sourceLabel);
          if (!srcId) {
            throw new Error(
              `Node "${label}" references non-existent node "${sourceLabel}" in needs.`
            );
          }
          const srcNode = nodeByLabel.get(sourceLabel);
          if (!srcNode) return;

          const srcOutputs = (srcNode.data.outputs || []) as Array<{
            name: string;
            produces_type: PacketType;
          }>;
          const sourceHandleName = sourcePin ?? srcOutputs[0]?.name;
          if (!sourceHandleName) {
            throw new Error(`Node "${sourceLabel}" has no output pins to connect to "${label}".`);
          }

          validateConnectionCompatibility(
            srcNode,
            targetNode,
            sourceHandleName,
            targetPin,
            sourceLabel,
            label,
            Array.from(nodeByLabel.values()),
            newEdges
          );

          createEdgeForConnection(
            srcNode,
            targetNode,
            srcId,
            targetId,
            sourceHandleName,
            targetPin,
            undefined,
            nodeByLabel,
            newEdges
          );
        });
        return;
      }

      const needs: NeedsDependency[] = Array.isArray(rawNeeds)
        ? rawNeeds
        : [rawNeeds as NeedsDependency];

      needs.forEach((dep: NeedsDependency, needsIndex: number) => {
        const { sourceLabel, sourcePin, mode } = parseDependency(dep);

        const sourceId = labelToIdMap.get(sourceLabel);

        if (!sourceId) {
          throw new Error(
            `Node "${label}" references non-existent node "${sourceLabel}" in needs.`
          );
        }

        const sourceNode = nodeByLabel.get(sourceLabel);
        if (!sourceNode) return;

        const sourceOutputs = (sourceNode.data.outputs || []) as Array<{
          name: string;
          produces_type: PacketType;
        }>;
        const targetInputs = (targetNode.data.inputs || []) as Array<{
          name: string;
          accepts_types: PacketType[];
        }>;

        const sourceHandleName = sourcePin ?? sourceOutputs[0]?.name;
        const targetHandleName = (targetInputs[needsIndex] || targetInputs[0])?.name;

        if (!sourceHandleName) {
          throw new Error(`Node "${sourceLabel}" has no output pins to connect to "${label}".`);
        }
        if (!targetHandleName) return;

        validateConnectionCompatibility(
          sourceNode,
          targetNode,
          sourceHandleName,
          targetHandleName,
          sourceLabel,
          label,
          Array.from(nodeByLabel.values()),
          newEdges
        );

        createEdgeForConnection(
          sourceNode,
          targetNode,
          sourceId,
          targetId,
          sourceHandleName,
          targetHandleName,
          mode,
          nodeByLabel,
          newEdges
        );
      });
    }
  );
}

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

    const modeError = validateMode(parsed.mode);
    if (modeError) {
      return { nodes: [], edges: [], mode: 'dynamic', error: modeError };
    }

    const detectedMode = detectMode(parsed);

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

    resetCounters();

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

    createEdgesFromPipeline(pipeline.nodes, labelToIdMap, nodeByLabel, newEdges);

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
