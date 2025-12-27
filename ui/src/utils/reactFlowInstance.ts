// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { ReactFlowInstance, Node, Edge } from '@xyflow/react';

const isFiniteNumber = (value: unknown): value is number =>
  typeof value === 'number' && Number.isFinite(value);

const extractNodeHeight = <
  NodeData extends Record<string, unknown>,
  NodeType extends string | undefined,
>(
  node: Node<NodeData, NodeType>
): number | undefined => {
  if (isFiniteNumber(node.height)) {
    return node.height;
  }

  if (isFiniteNumber(node.measured?.height)) {
    return node.measured?.height;
  }

  if (isFiniteNumber(node.initialHeight)) {
    return node.initialHeight;
  }

  return undefined;
};

export const collectNodeHeights = <
  NodeData extends Record<string, unknown> = Record<string, unknown>,
  NodeType extends string | undefined = string | undefined,
  EdgeData extends Record<string, unknown> = Record<string, unknown>,
  EdgeType extends string | undefined = string | undefined,
>(
  instance: ReactFlowInstance<Node<NodeData, NodeType>, Edge<EdgeData, EdgeType>> | null | undefined
): Record<string, number> => {
  const heights: Record<string, number> = {};
  if (!instance) {
    return heights;
  }

  const nodes = instance.getNodes();

  nodes.forEach((node) => {
    const height = extractNodeHeight(node);
    if (isFiniteNumber(height)) {
      heights[node.id] = height;
    }
  });

  return heights;
};
