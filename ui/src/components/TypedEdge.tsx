// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { BaseEdge, getBezierPath, type EdgeProps } from '@xyflow/react';
import React from 'react';

import type { PacketType } from '@/types/types';
import { getPacketTypeColor } from '@/utils/packetTypes';

export type TypedEdgeData = {
  resolvedType?: PacketType;
  [key: string]: unknown;
};

const TypedEdge: React.FC<EdgeProps> = ({
  sourceX,
  sourceY,
  targetX,
  targetY,
  sourcePosition,
  targetPosition,
  style = {},
  data,
}) => {
  const [edgePath] = getBezierPath({
    sourceX,
    sourceY,
    sourcePosition,
    targetX,
    targetY,
    targetPosition,
  });

  const resolvedType = (data as TypedEdgeData | undefined)?.resolvedType;

  // Get color based on resolved type
  const typeColor = resolvedType ? getPacketTypeColor(resolvedType) : 'var(--sk-primary)';

  // Override style with type-specific color
  const edgeStyle: React.CSSProperties = {
    ...(style || {}),
    stroke: typeColor,
  };

  return <BaseEdge path={edgePath} style={edgeStyle} />;
};

export default TypedEdge;
