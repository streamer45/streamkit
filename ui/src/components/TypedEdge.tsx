// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { BaseEdge, EdgeLabelRenderer, getBezierPath, type EdgeProps } from '@xyflow/react';
import React from 'react';

import { SKTooltip } from '@/components/Tooltip';
import type { PacketType } from '@/types/types';
import { getPacketTypeColor } from '@/utils/packetTypes';

export type TypedEdgeData = {
  resolvedType?: PacketType;
  alert?: {
    kind: string;
    severity: 'warning' | 'error';
    tooltip?: {
      title: string;
      lines: string[];
    };
  };
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
  const [edgePath, labelX, labelY] = getBezierPath({
    sourceX,
    sourceY,
    sourcePosition,
    targetX,
    targetY,
    targetPosition,
  });

  const resolvedType = (data as TypedEdgeData | undefined)?.resolvedType;
  const alert = (data as TypedEdgeData | undefined)?.alert;

  // Get color based on resolved type
  const typeColor = resolvedType ? getPacketTypeColor(resolvedType) : 'var(--sk-primary)';

  const alertColor =
    alert?.severity === 'error'
      ? 'var(--sk-danger)'
      : alert?.severity === 'warning'
        ? 'var(--sk-warning)'
        : null;

  // Override style with type-specific color
  const edgeStyle: React.CSSProperties = {
    ...(style || {}),
    stroke: alertColor ?? typeColor,
    strokeDasharray: alertColor
      ? '6, 4'
      : (style as React.CSSProperties | undefined)?.strokeDasharray,
    strokeWidth: alertColor ? 3 : (style as React.CSSProperties | undefined)?.strokeWidth,
  };

  const badgeIcon =
    alert?.severity === 'error'
      ? '❌'
      : alert?.severity === 'warning' && alert?.kind === 'slow_input_timeout'
        ? '⏱️'
        : alert
          ? '⚠️'
          : null;

  const tooltipContent =
    alert?.tooltip && badgeIcon ? (
      <div style={{ fontSize: 12 }}>
        <div style={{ fontWeight: 600, marginBottom: 4 }}>
          {badgeIcon} {alert.tooltip.title}
        </div>
        {alert.tooltip.lines.map((line) => (
          <div key={line} className="code-font" style={{ fontSize: 11, lineHeight: '1.4' }}>
            {line}
          </div>
        ))}
      </div>
    ) : null;

  return (
    <>
      <BaseEdge path={edgePath} style={edgeStyle} />
      {alertColor && badgeIcon && (
        <EdgeLabelRenderer>
          <SKTooltip content={tooltipContent} side="top">
            <div
              className="nodrag nopan"
              style={{
                position: 'absolute',
                transform: `translate(-50%, -50%) translate(${labelX}px, ${labelY}px)`,
                background: alertColor,
                color: 'white',
                border: '1px solid var(--sk-border-strong)',
                borderRadius: 999,
                padding: '2px 6px',
                fontSize: 12,
                fontWeight: 700,
                pointerEvents: 'auto',
                boxShadow: '0 2px 10px var(--sk-shadow)',
              }}
            >
              {badgeIcon}
            </div>
          </SKTooltip>
        </EdgeLabelRenderer>
      )}
    </>
  );
};

export default TypedEdge;
