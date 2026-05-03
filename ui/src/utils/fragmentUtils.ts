// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { Node, Edge } from '@xyflow/react';

import type { NodeDefinition } from '@/types/generated/api-types';
import { utilsLogger } from '@/utils/logger';

let idCounter = 1;
export const generateNodeId = () => `skitnode_${idCounter++}`;

export interface FragmentNode {
  kind: string;
  params?: Record<string, unknown>;
  needs?: string | string[];
}

export interface FragmentData {
  nodes: Record<string, FragmentNode>;
}

export function fragmentToReactFlow(
  fragment: FragmentData,
  position: { x: number; y: number },
  nodeDefinitions: NodeDefinition[],
  handlers: {
    onParamChange: (nodeId: string, paramName: string, value: unknown) => void;
    onLabelChange: (nodeId: string, newLabel: string) => void;
  },
  nextLabelForKind: (kind: string) => string
): {
  nodes: Node<Record<string, unknown>>[];
  edges: Edge[];
  idMapping: Map<string, string>; // old label -> new ID
} {
  const idMapping = new Map<string, string>();
  const nodes: Node<Record<string, unknown>>[] = [];
  const edges: Edge[] = [];

  const fragmentNodeLabels = Object.keys(fragment.nodes);
  const gridColumns = Math.ceil(Math.sqrt(fragmentNodeLabels.length));
  const nodeSpacing = { x: 200, y: 150 };

  fragmentNodeLabels.forEach((oldLabel, index) => {
    const fragmentNode = fragment.nodes[oldLabel];
    const nodeDefinition = nodeDefinitions.find((def) => def.kind === fragmentNode.kind);

    const newId = generateNodeId();
    idMapping.set(oldLabel, newId);

    const row = Math.floor(index / gridColumns);
    const col = index % gridColumns;
    const nodePosition = {
      x: position.x + col * nodeSpacing.x,
      y: position.y + row * nodeSpacing.y,
    };

    let nodeType = 'configurable';
    if (fragmentNode.kind === 'audio::gain') {
      nodeType = 'audioGain';
    }

    const newNode: Node = {
      id: newId,
      type: nodeType,
      dragHandle: '.drag-handle',
      position: nodePosition,
      data: {
        label: nextLabelForKind(fragmentNode.kind),
        kind: fragmentNode.kind,
        params: fragmentNode.params || {},
        paramSchema: nodeDefinition?.param_schema,
        inputs: nodeDefinition?.inputs || [],
        outputs: nodeDefinition?.outputs || [],
        nodeDefinition: nodeDefinition,
        definition: { bidirectional: nodeDefinition?.bidirectional },
        onParamChange: handlers.onParamChange,
        onLabelChange: handlers.onLabelChange,
      },
      selected: false,
    };

    nodes.push(newNode);
  });

  fragmentNodeLabels.forEach((targetLabel) => {
    const fragmentNode = fragment.nodes[targetLabel];
    const targetId = idMapping.get(targetLabel);

    if (!fragmentNode.needs || !targetId) return;

    const dependencies = Array.isArray(fragmentNode.needs)
      ? fragmentNode.needs
      : [fragmentNode.needs];

    dependencies.forEach((sourceLabel) => {
      const sourceId = idMapping.get(sourceLabel);

      if (!sourceId) {
        utilsLogger.warn(
          `[FragmentUtils] Could not find node mapping for dependency: ${sourceLabel} -> ${targetLabel}`
        );
        return;
      }

      const newEdge: Edge = {
        id: `${sourceId}_out_${targetId}_in`,
        source: sourceId,
        sourceHandle: 'out',
        target: targetId,
        targetHandle: 'in',
        type: 'default',
      };

      edges.push(newEdge);
    });
  });

  return { nodes, edges, idMapping };
}

export function extractFragment(
  selectedNodeIds: string[],
  allNodes: Node[],
  allEdges: Edge[]
): FragmentData {
  const selectedSet = new Set(selectedNodeIds);
  const nodes: Record<string, FragmentNode> = {};

  const incomingEdges = new Map<string, string[]>();
  allEdges.forEach((edge) => {
    if (selectedSet.has(edge.source) && selectedSet.has(edge.target)) {
      const sourceNode = allNodes.find((n) => n.id === edge.source);
      if (!sourceNode) return;

      const sourceLabel = String(sourceNode.data.label || sourceNode.id);
      const existing = incomingEdges.get(edge.target) || [];
      existing.push(sourceLabel);
      incomingEdges.set(edge.target, existing);
    }
  });

  allNodes.forEach((node) => {
    if (selectedSet.has(node.id)) {
      const label = String(node.data.label || node.id);
      const kind = String(node.data.kind || 'unknown');

      const fragmentNode: FragmentNode = {
        kind,
        params: (node.data.params as Record<string, unknown>) || {},
      };

      const dependencies = incomingEdges.get(node.id);
      if (dependencies && dependencies.length > 0) {
        fragmentNode.needs = dependencies.length === 1 ? dependencies[0] : dependencies;
      }

      nodes[label] = fragmentNode;
    }
  });

  return { nodes };
}
