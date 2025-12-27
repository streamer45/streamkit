// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { ValidationError } from '@/stores/stagingStore';
import type { Connection, Pipeline, NodeDefinition, PacketType } from '@/types/types';
import { wouldCreateCycle } from '@/utils/dag';

/**
 * Validates that all nodes in the pipeline exist in the registry
 */
function validateNodeDefinitions(
  pipeline: Pipeline,
  nodeDefinitions: Map<string, NodeDefinition>
): ValidationError[] {
  const errors: ValidationError[] = [];

  for (const [nodeId, node] of Object.entries(pipeline.nodes)) {
    const def = nodeDefinitions.get(node.kind);
    if (!def) {
      errors.push({
        type: 'error',
        message: `Unknown node type: ${node.kind}`,
        nodeId,
      });
    }
  }

  return errors;
}

/**
 * Validates connection endpoints (nodes exist)
 */
function validateConnectionEndpoints(conn: Connection, pipeline: Pipeline): ValidationError | null {
  const fromNode = pipeline.nodes[conn.from_node];
  if (!fromNode) {
    return {
      type: 'error',
      message: `Connection source node not found: ${conn.from_node}`,
      connectionId: connectionKey(conn),
    };
  }

  const toNode = pipeline.nodes[conn.to_node];
  if (!toNode) {
    return {
      type: 'error',
      message: `Connection target node not found: ${conn.to_node}`,
      connectionId: connectionKey(conn),
    };
  }

  return null;
}

/**
 * Resolves an output pin (including dynamic pins)
 */
function resolveOutputPin(
  conn: Connection,
  fromDef: NodeDefinition,
  fromNode: { kind: string }
): {
  pin: { name: string; produces_type: PacketType; cardinality: unknown } | null;
  error: ValidationError | null;
} {
  let outputPin = fromDef.outputs.find((p) => p.name === conn.from_pin);

  if (!outputPin) {
    const hasDynamicOutputs = fromDef.outputs.some(
      (pin) => typeof pin.cardinality === 'object' && 'Dynamic' in pin.cardinality
    );
    const isDynamicPinName = /^out_\d+$/.test(conn.from_pin);

    if (hasDynamicOutputs && isDynamicPinName) {
      const dynamicTemplate = fromDef.outputs.find(
        (pin) => typeof pin.cardinality === 'object' && 'Dynamic' in pin.cardinality
      );
      if (dynamicTemplate) {
        outputPin = {
          name: conn.from_pin,
          produces_type: dynamicTemplate.produces_type,
          cardinality: 'One',
        };
      }
    }

    if (!outputPin) {
      return {
        pin: null,
        error: {
          type: 'error',
          message: `Node ${conn.from_node} (${fromNode.kind}) does not have output pin: ${conn.from_pin}`,
          connectionId: connectionKey(conn),
        },
      };
    }
  }

  return { pin: outputPin, error: null };
}

/**
 * Resolves an input pin (including dynamic pins)
 */
function resolveInputPin(
  conn: Connection,
  toDef: NodeDefinition,
  toNode: { kind: string }
): {
  pin: { name: string; accepts_types: PacketType[]; cardinality: unknown } | null;
  error: ValidationError | null;
} {
  let inputPin = toDef.inputs.find((p) => p.name === conn.to_pin);

  if (!inputPin) {
    const hasDynamicInputs = toDef.inputs.some(
      (pin) => typeof pin.cardinality === 'object' && 'Dynamic' in pin.cardinality
    );
    const isDynamicPinName = /^in_\d+$/.test(conn.to_pin);

    if (hasDynamicInputs && isDynamicPinName) {
      const dynamicTemplate = toDef.inputs.find(
        (pin) => typeof pin.cardinality === 'object' && 'Dynamic' in pin.cardinality
      );
      if (dynamicTemplate) {
        inputPin = {
          name: conn.to_pin,
          accepts_types: dynamicTemplate.accepts_types,
          cardinality: 'One',
        };
      }
    }

    if (!inputPin) {
      return {
        pin: null,
        error: {
          type: 'error',
          message: `Node ${conn.to_node} (${toNode.kind}) does not have input pin: ${conn.to_pin}`,
          connectionId: connectionKey(conn),
        },
      };
    }
  }

  return { pin: inputPin, error: null };
}

/**
 * Validates a single connection
 */
function validateConnection(
  conn: Connection,
  pipeline: Pipeline,
  nodeDefinitions: Map<string, NodeDefinition>,
  inferredTypes: Map<string, PacketType>
): ValidationError[] {
  const errors: ValidationError[] = [];

  // Check if nodes exist
  const endpointError = validateConnectionEndpoints(conn, pipeline);
  if (endpointError) {
    return [endpointError];
  }

  const fromNode = pipeline.nodes[conn.from_node];
  const toNode = pipeline.nodes[conn.to_node];
  const fromDef = nodeDefinitions.get(fromNode.kind);
  const toDef = nodeDefinitions.get(toNode.kind);

  if (!fromDef || !toDef) {
    return []; // Already flagged as unknown node type
  }

  // Resolve output pin
  const { pin: outputPin, error: outputError } = resolveOutputPin(conn, fromDef, fromNode);
  if (outputError) {
    return [outputError];
  }
  if (!outputPin) return [];

  // Resolve input pin
  const { pin: inputPin, error: inputError } = resolveInputPin(conn, toDef, toNode);
  if (inputError) {
    return [inputError];
  }
  if (!inputPin) return [];

  // Resolve output type (handle Passthrough type inference)
  const resolvedOutputType = resolvePassthroughType(
    outputPin.produces_type,
    conn.from_node,
    conn.from_pin,
    inferredTypes
  );

  // Check pin type compatibility
  const typeError = validatePinTypes(resolvedOutputType, inputPin.accepts_types);
  if (typeError) {
    errors.push({
      type: 'error',
      message: `Incompatible pin types: ${conn.from_node}:${conn.from_pin} → ${conn.to_node}:${conn.to_pin}. ${typeError}`,
      connectionId: connectionKey(conn),
    });
  }

  return errors;
}

/**
 * Validates a pipeline for common errors before committing changes.
 * Returns an array of validation errors. Empty array means valid pipeline.
 */
export function validatePipeline(
  pipeline: Pipeline,
  nodeDefinitions: Map<string, NodeDefinition>
): ValidationError[] {
  const errors: ValidationError[] = [];

  // Validate each node exists in registry
  errors.push(...validateNodeDefinitions(pipeline, nodeDefinitions));

  // Build a type inference map for Passthrough nodes
  const inferredTypes = inferPassthroughTypes(pipeline, nodeDefinitions);

  // Validate each connection
  for (const conn of pipeline.connections) {
    const connectionErrors = validateConnection(conn, pipeline, nodeDefinitions, inferredTypes);
    errors.push(...connectionErrors);
  }

  // Check for cycles using existing dag utility
  errors.push(...detectCyclesWithDagUtil(pipeline, nodeDefinitions));

  // Warn about disconnected nodes (non-fatal)
  errors.push(...detectDisconnectedNodes(pipeline, nodeDefinitions));

  return errors;
}

/**
 * Infers the actual types for Passthrough nodes by tracing back through the pipeline.
 * Returns a map of "nodeId:pinName" -> inferred PacketType.
 */
function inferPassthroughTypes(
  pipeline: Pipeline,
  nodeDefinitions: Map<string, NodeDefinition>
): Map<string, PacketType> {
  const inferredTypes = new Map<string, PacketType>();
  const visited = new Set<string>();

  // Helper function to get the input type for a node's input pin
  function getInputType(nodeId: string, inputPinName: string): PacketType | null {
    // Find the connection that feeds this input pin
    const incomingConn = pipeline.connections.find(
      (c) => c.to_node === nodeId && c.to_pin === inputPinName
    );

    if (!incomingConn) {
      return null; // No input connection
    }

    const fromNode = pipeline.nodes[incomingConn.from_node];
    if (!fromNode) return null;

    const fromDef = nodeDefinitions.get(fromNode.kind);
    if (!fromDef) return null;

    const outputPin = fromDef.outputs.find((p) => p.name === incomingConn.from_pin);
    if (!outputPin) return null;

    // If the source is also Passthrough, recursively resolve it
    if (outputPin.produces_type === 'Passthrough') {
      return resolvePassthroughTypeRecursive(
        incomingConn.from_node,
        incomingConn.from_pin,
        visited,
        inferredTypes
      );
    }

    return outputPin.produces_type;
  }

  // Recursive function to resolve Passthrough types
  function resolvePassthroughTypeRecursive(
    nodeId: string,
    outputPinName: string,
    visited: Set<string>,
    inferredTypes: Map<string, PacketType>
  ): PacketType | null {
    const key = `${nodeId}:${outputPinName}`;

    // Check if already resolved
    if (inferredTypes.has(key)) {
      return inferredTypes.get(key)!;
    }

    // Prevent infinite recursion
    if (visited.has(key)) {
      return null; // Circular dependency
    }

    visited.add(key);

    const node = pipeline.nodes[nodeId];
    if (!node) {
      visited.delete(key);
      return null;
    }

    const nodeDef = nodeDefinitions.get(node.kind);
    if (!nodeDef) {
      visited.delete(key);
      return null;
    }

    const outputPin = nodeDef.outputs.find((p) => p.name === outputPinName);
    if (!outputPin || outputPin.produces_type !== 'Passthrough') {
      visited.delete(key);
      return outputPin?.produces_type ?? null;
    }

    // This is a Passthrough node - find its input type
    // For single-input passthrough nodes, use the first input pin
    if (nodeDef.inputs.length > 0) {
      const inputPinName = nodeDef.inputs[0].name;
      const inputType = getInputType(nodeId, inputPinName);

      if (inputType) {
        inferredTypes.set(key, inputType);
        visited.delete(key);
        return inputType;
      }
    }

    visited.delete(key);
    return null;
  }

  // Pre-compute types for all Passthrough nodes
  precomputePassthroughTypes(
    pipeline,
    nodeDefinitions,
    inferredTypes,
    resolvePassthroughTypeRecursive
  );

  return inferredTypes;
}

/**
 * Processes output pins for a single node to infer Passthrough types
 */
function processNodePassthroughPins(
  nodeId: string,
  nodeDef: NodeDefinition,
  inferredTypes: Map<string, PacketType>,
  resolvePassthroughTypeRecursive: (
    nodeId: string,
    outputPinName: string,
    visited: Set<string>,
    inferredTypes: Map<string, PacketType>
  ) => PacketType | null
): void {
  for (const outputPin of nodeDef.outputs) {
    if (outputPin.produces_type !== 'Passthrough') continue;

    const key = `${nodeId}:${outputPin.name}`;
    if (inferredTypes.has(key)) continue;

    const inferredType = resolvePassthroughTypeRecursive(
      nodeId,
      outputPin.name,
      new Set(),
      inferredTypes
    );
    if (inferredType) {
      inferredTypes.set(key, inferredType);
    }
  }
}

/**
 * Pre-computes inferred types for all Passthrough output pins
 */
function precomputePassthroughTypes(
  pipeline: Pipeline,
  nodeDefinitions: Map<string, NodeDefinition>,
  inferredTypes: Map<string, PacketType>,
  resolvePassthroughTypeRecursive: (
    nodeId: string,
    outputPinName: string,
    visited: Set<string>,
    inferredTypes: Map<string, PacketType>
  ) => PacketType | null
): void {
  for (const [nodeId, node] of Object.entries(pipeline.nodes)) {
    const nodeDef = nodeDefinitions.get(node.kind);
    if (!nodeDef) continue;

    processNodePassthroughPins(nodeId, nodeDef, inferredTypes, resolvePassthroughTypeRecursive);
  }
}

/**
 * Resolves a Passthrough type to its actual inferred type.
 */
function resolvePassthroughType(
  outputType: PacketType,
  nodeId: string,
  pinName: string,
  inferredTypes: Map<string, PacketType>
): PacketType {
  if (outputType === 'Passthrough') {
    const key = `${nodeId}:${pinName}`;
    const inferred = inferredTypes.get(key);
    if (inferred) {
      return inferred;
    }
    // Fallback to Any if inference failed (e.g., circular dependency or missing input)
    return 'Any';
  }
  return outputType;
}

/**
 * Checks if the output pin type is compatible with any of the accepted input types.
 * Returns error message if incompatible, null if compatible.
 */
function validatePinTypes(outputType: PacketType, acceptedTypes: PacketType[]): string | null {
  // Special case: "Any" type accepts everything
  for (const acceptedType of acceptedTypes) {
    if (isTypeAny(acceptedType)) {
      return null;
    }
  }

  // Special case: If output is "Any", it may not match specific input requirements
  if (isTypeAny(outputType)) {
    return null; // Allow for now, but could be made stricter
  }

  // Check for exact match or compatible match
  for (const acceptedType of acceptedTypes) {
    if (typesMatch(outputType, acceptedType)) {
      return null;
    }
  }

  return `Output type ${formatPacketType(outputType)} not compatible with accepted types: ${acceptedTypes.map(formatPacketType).join(', ')}`;
}

/**
 * Checks if a PacketType is "Any"
 */
function isTypeAny(type: PacketType): type is 'Any' {
  return type === 'Any';
}

/**
 * Checks if two packet types match
 */
function typesMatch(outputType: PacketType, acceptedType: PacketType): boolean {
  // Handle string enum types
  if (typeof outputType === 'string' && typeof acceptedType === 'string') {
    return outputType === acceptedType;
  }

  // Handle RawAudio with specific format
  if (
    typeof outputType === 'object' &&
    typeof acceptedType === 'object' &&
    'RawAudio' in outputType &&
    'RawAudio' in acceptedType
  ) {
    // For now, accept any RawAudio format matching
    // Could be made stricter to check sample_rate, channels, etc.
    return true;
  }

  return false;
}

/**
 * Formats a PacketType for display in error messages
 */
function formatPacketType(type: PacketType): string {
  if (typeof type === 'string') {
    if (type === 'Passthrough') {
      return 'Passthrough (inferred)';
    }
    return type;
  }

  if (typeof type === 'object') {
    if ('RawAudio' in type) {
      const format = (
        type as { RawAudio: { sample_rate: number; channels: number; sample_format: string } }
      ).RawAudio;
      return `RawAudio(${format.sample_rate}Hz, ${format.channels}ch, ${format.sample_format})`;
    }
  }

  return JSON.stringify(type);
}

/**
 * Detects cycles in the pipeline graph using the existing dag utility.
 * Bidirectional nodes (like moq_peer) are allowed to have cycles.
 */
function detectCyclesWithDagUtil(
  pipeline: Pipeline,
  nodeDefinitions: Map<string, NodeDefinition>
): ValidationError[] {
  const errors: ValidationError[] = [];

  // Build list of bidirectional node IDs
  const bidirectionalNodeIds: string[] = [];
  for (const [nodeId, node] of Object.entries(pipeline.nodes)) {
    const def = nodeDefinitions.get(node.kind);
    if (def?.bidirectional) {
      bidirectionalNodeIds.push(nodeId);
    }
  }

  const nodeIds = Object.keys(pipeline.nodes);
  const edges = pipeline.connections.map((conn) => ({
    source: conn.from_node,
    target: conn.to_node,
  }));

  // Check each potential edge (we're validating an existing pipeline, not adding a new edge)
  // So we iterate through all edges and check if removing them would make the graph acyclic
  for (const conn of pipeline.connections) {
    const edgesWithoutCurrent = edges.filter(
      (e) => !(e.source === conn.from_node && e.target === conn.to_node)
    );

    if (
      wouldCreateCycle(
        nodeIds,
        edgesWithoutCurrent,
        { source: conn.from_node, target: conn.to_node },
        bidirectionalNodeIds
      )
    ) {
      errors.push({
        type: 'error',
        message: `Connection creates a cycle: ${conn.from_node} → ${conn.to_node}`,
        connectionId: connectionKey(conn),
      });
    }
  }

  return errors;
}

/**
 * Detects nodes that are disconnected (no inputs or outputs where expected)
 * Returns warnings (non-fatal)
 */
function detectDisconnectedNodes(
  pipeline: Pipeline,
  nodeDefinitions: Map<string, NodeDefinition>
): ValidationError[] {
  const warnings: ValidationError[] = [];

  for (const [nodeId, node] of Object.entries(pipeline.nodes)) {
    const def = nodeDefinitions.get(node.kind);
    if (!def) continue; // Already flagged elsewhere

    const hasInputs = def.inputs.length > 0;
    const hasOutputs = def.outputs.length > 0;

    const incomingConnections = pipeline.connections.filter((c) => c.to_node === nodeId);
    const outgoingConnections = pipeline.connections.filter((c) => c.from_node === nodeId);

    // Warn if node expects inputs but has none
    if (hasInputs && incomingConnections.length === 0) {
      warnings.push({
        type: 'warning',
        message: `Node ${nodeId} (${node.kind}) has no incoming connections`,
        nodeId,
      });
    }

    // Warn if node produces outputs but has none connected
    if (hasOutputs && outgoingConnections.length === 0) {
      warnings.push({
        type: 'warning',
        message: `Node ${nodeId} (${node.kind}) has no outgoing connections`,
        nodeId,
      });
    }
  }

  return warnings;
}

/**
 * Creates a unique key for a connection
 */
function connectionKey(conn: Connection): string {
  return `${conn.from_node}:${conn.from_pin}→${conn.to_node}:${conn.to_pin}`;
}
