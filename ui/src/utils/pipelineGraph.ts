// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Pure helper functions for converting Pipeline data into ReactFlow
 * nodes and edges.  Used by the Monitor View's topology effect.
 */

import type { Node as RFNode, Edge } from '@xyflow/react';
import { dump } from 'js-yaml';

import type {
  Connection,
  Node,
  NodeDefinition,
  NodeState,
  Pipeline,
  InputPin,
  OutputPin,
} from '@/types/types';

// ---------------------------------------------------------------------------
// Edge-alert helpers (slow-input-timeout)
// ---------------------------------------------------------------------------

export type SlowTimeoutDetails = {
  slowPins: string[];
  newlySlowPins: string[];
  syncTimeoutMs: number | null;
};

export const isRecord = (value: unknown): value is Record<string, unknown> =>
  value !== null && value !== undefined && typeof value === 'object' && !Array.isArray(value);

export const extractSlowTimeoutDetailsFromNodeState = (
  state: NodeState | null | undefined
): SlowTimeoutDetails | null => {
  if (!state || typeof state === 'string') return null;
  if (!('Degraded' in state)) return null;
  if (state.Degraded.reason !== 'slow_input_timeout') return null;

  const details = state.Degraded.details;
  if (!isRecord(details)) return null;

  const slowPinsRaw = details['slow_pins'];
  const newlySlowPinsRaw = details['newly_slow_pins'];
  const syncTimeoutRaw = details['sync_timeout_ms'];

  const slowPins = Array.isArray(slowPinsRaw)
    ? slowPinsRaw.filter((p): p is string => typeof p === 'string')
    : [];
  const newlySlowPins = Array.isArray(newlySlowPinsRaw)
    ? newlySlowPinsRaw.filter((p): p is string => typeof p === 'string')
    : [];
  const syncTimeoutMs = typeof syncTimeoutRaw === 'number' ? syncTimeoutRaw : null;

  return { slowPins, newlySlowPins, syncTimeoutMs };
};

export const describeSlowInputs = (
  pipeline: Pipeline,
  nodeId: string,
  slowPins: string[]
): string[] => {
  if (slowPins.length === 0) return [];
  const slowPinSet = new Set(slowPins);

  const sources = pipeline.connections
    .filter((c) => c.to_node === nodeId && slowPinSet.has(c.to_pin))
    .map((c) => `${c.from_node}.${c.from_pin} → ${c.to_pin}`);

  sources.sort();
  return sources;
};

// ---------------------------------------------------------------------------
// Edge connection validation
// ---------------------------------------------------------------------------

/**
 * Checks if an edge connection is valid (both source and target pins exist).
 * Prevents React Flow warnings about missing handles.
 */
const isValidEdgeConnection = (conn: Connection, nodeMap: Map<string, RFNode>): boolean => {
  const sourceNode = nodeMap.get(conn.from_node);
  const targetNode = nodeMap.get(conn.to_node);

  if (!sourceNode || !targetNode) return false;

  const isDynamicTemplatePin = (pin: InputPin | OutputPin): boolean =>
    typeof pin.cardinality === 'object' && pin.cardinality !== null && 'Dynamic' in pin.cardinality;

  // Check if the output pin exists
  const sourceOutputs = (sourceNode.data.outputs || []) as OutputPin[];
  const hasSourcePin = sourceOutputs.some(
    (pin) => pin.name === conn.from_pin && !isDynamicTemplatePin(pin)
  );

  // Check if the input pin exists
  const targetInputs = (targetNode.data.inputs || []) as InputPin[];
  const hasTargetPin = targetInputs.some(
    (pin) => pin.name === conn.to_pin && !isDynamicTemplatePin(pin)
  );

  return hasSourcePin && hasTargetPin;
};

/**
 * Build edges from pipeline connections, filtering out invalid ones.
 */
export const buildEdgesFromConnections = (connections: Connection[], nodes: RFNode[]): Edge[] => {
  const nodeMap = new Map(nodes.map((n) => [n.id, n]));

  return connections
    .filter((conn) => isValidEdgeConnection(conn, nodeMap))
    .map((conn) => ({
      id: `${conn.from_node}_${conn.from_pin}-${conn.to_node}_${conn.to_pin}`,
      source: conn.from_node,
      sourceHandle: conn.from_pin,
      target: conn.to_node,
      targetHandle: conn.to_pin,
    }));
};

// ---------------------------------------------------------------------------
// YAML generation
// ---------------------------------------------------------------------------

/**
 * Generate YAML representation of the pipeline ordered by topological sort.
 */
export const generatePipelineYaml = (pipeline: Pipeline, orderedNames: string[]): string => {
  const yamlObject: { nodes: Record<string, unknown> } = { nodes: {} };

  for (const nodeName of orderedNames) {
    const apiNode = pipeline.nodes[nodeName];
    if (!apiNode) continue;

    const needs = pipeline.connections
      .filter((c: Connection) => c.to_node === nodeName)
      .map((c: Connection) => c.from_node);

    const nodeConfig: Record<string, unknown> = { kind: apiNode.kind };
    if (apiNode.params && Object.keys(apiNode.params).length > 0) {
      nodeConfig['params'] = apiNode.params;
    }
    if (needs.length === 1) {
      nodeConfig['needs'] = needs[0];
    } else if (needs.length > 1) {
      nodeConfig['needs'] = needs;
    }
    yamlObject.nodes[nodeName] = nodeConfig;
  }

  return dump(yamlObject, { skipInvalid: true });
};

// ---------------------------------------------------------------------------
// ReactFlow node construction
// ---------------------------------------------------------------------------

/**
 * Build a single Node object from pipeline data.
 * Helper for topology effect to reduce complexity.
 */
export interface BuildNodeParams {
  nodeName: string;
  apiNode: Node;
  position: { x: number; y: number };
  nodeState: unknown; // Can be string | null or NodeState enum
  finalInputs: InputPin[];
  finalOutputs: OutputPin[];
  nodeDef: NodeDefinition | undefined;
  stableOnParamChange: (nodeId: string, paramName: string, value: unknown) => void;
  stableOnConfigChange?: (nodeId: string, config: Record<string, unknown>) => void;
  selectedSessionId: string | null;
}

/** Determine the ReactFlow node type from the pipeline node kind */
export const nodeTypeForKind = (kind: string): string => {
  if (kind === 'audio::gain') return 'audioGain';
  if (kind === 'video::compositor') return 'compositor';
  return 'configurable';
};

export const buildNodeObject = (params: BuildNodeParams): RFNode => {
  return {
    id: params.nodeName,
    type: nodeTypeForKind(params.apiNode.kind),
    position: params.position,
    dragHandle: '.drag-handle',
    data: {
      label: params.nodeName,
      kind: params.apiNode.kind,
      params: params.apiNode.params || {},
      inputs: params.finalInputs,
      outputs: params.finalOutputs,
      paramSchema: params.nodeDef?.param_schema,
      nodeDefinition: params.nodeDef,
      definition: { bidirectional: params.nodeDef?.bidirectional },
      state: params.nodeState,
      // Stats are NOT included here to prevent re-renders when they update
      // NodeStateIndicator will fetch them directly from session store on hover
      onParamChange: params.stableOnParamChange,
      // Full-config change callback for compositor nodes
      onConfigChange: params.stableOnConfigChange,
      sessionId: params.selectedSessionId || undefined,
    },
  };
};
