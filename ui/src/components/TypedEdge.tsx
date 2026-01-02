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

type TypedEdgeAlert = NonNullable<TypedEdgeData['alert']>;

function getTypeColor(resolvedType: PacketType | undefined): string {
  return resolvedType ? getPacketTypeColor(resolvedType) : 'var(--sk-primary)';
}

function getAlertColor(alert: TypedEdgeAlert | undefined): string | null {
  if (!alert) return null;
  if (alert.severity === 'error') return 'var(--sk-danger)';
  if (alert.severity === 'warning') return 'var(--sk-warning)';
  return null;
}

function getBadgeIcon(alert: TypedEdgeAlert | undefined): string | null {
  if (!alert) return null;
  if (alert.severity === 'error') return '❌';
  if (alert.kind === 'slow_input_timeout') return '⏱️';
  return '⚠️';
}

function buildEdgeStyle(
  style: EdgeProps['style'] | undefined,
  typeColor: string,
  alertColor: string | null
): React.CSSProperties {
  const baseStyle = (style || {}) as React.CSSProperties;
  const next: React.CSSProperties = { ...baseStyle, stroke: alertColor ?? typeColor };

  if (alertColor) {
    next.strokeDasharray = '6, 4';
    next.strokeWidth = 3;
  }

  return next;
}

function renderAlertTooltip(alert: TypedEdgeAlert | undefined, badgeIcon: string | null) {
  if (!alert?.tooltip || !badgeIcon) return null;

  return (
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
  );
}

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

  const typedData = data as TypedEdgeData | undefined;
  const resolvedType = typedData?.resolvedType;
  const alert = typedData?.alert;

  const typeColor = getTypeColor(resolvedType);
  const alertColor = getAlertColor(alert);
  const edgeStyle = buildEdgeStyle(style, typeColor, alertColor);
  const badgeIcon = getBadgeIcon(alert);
  const tooltipContent = renderAlertTooltip(alert, badgeIcon);
  const shouldRenderBadge = !!alertColor && !!badgeIcon;

  return (
    <>
      <BaseEdge path={edgePath} style={edgeStyle} />
      {shouldRenderBadge && (
        <EdgeLabelRenderer>
          <SKTooltip content={tooltipContent} side="top">
            <div
              className="nodrag nopan"
              style={{
                position: 'absolute',
                transform: `translate(-50%, -50%) translate(${labelX}px, ${labelY}px)`,
                background: alertColor!,
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
