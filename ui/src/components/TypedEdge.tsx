// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import {
  BaseEdge,
  EdgeLabelRenderer,
  getBezierPath,
  type Edge,
  type EdgeProps,
} from '@xyflow/react';
import { atom, type Atom } from 'jotai';
import { useAtomValue } from 'jotai/react';
import { selectAtom } from 'jotai/utils';
import React from 'react';

import { SKTooltip } from '@/components/Tooltip';
import { nodeKey, nodeStateAtom } from '@/stores/sessionAtoms';
import type { PacketType } from '@/types/types';
import { deepEqual } from '@/utils/deepEqual';
import { getPacketTypeColor } from '@/utils/packetTypes';
import {
  describeSlowInputsFromConnections,
  extractSlowTimeoutDetailsFromNodeState,
  type MonitorEdgeAlertContext,
  type SlowTimeoutDetails,
} from '@/utils/pipelineGraph';

export type TypedEdgeData = {
  resolvedType?: PacketType;
  monitorAlertContext?: MonitorEdgeAlertContext;
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
type SlowInputDetailsAtom = Atom<SlowTimeoutDetails | null>;
type AlertEdge = Pick<Edge, 'source' | 'sourceHandle' | 'target' | 'targetHandle'>;

const nullSlowInputDetailsAtom = atom<SlowTimeoutDetails | null>(null);

function buildSlowInputTooltipLines(
  edge: AlertEdge,
  details: SlowTimeoutDetails,
  connections: MonitorEdgeAlertContext['connections']
): string[] {
  const slowInputs = describeSlowInputsFromConnections(connections, edge.target, details.slowPins);
  const lines: string[] = [];
  if (slowInputs.length > 0) {
    lines.push(`Slow inputs: ${slowInputs.join(', ')}`);
  } else if (details.slowPins.length > 0) {
    lines.push(`Slow pins: ${details.slowPins.join(', ')}`);
  }

  lines.push(`This: ${edge.source}.${edge.sourceHandle ?? ''} → ${edge.targetHandle ?? ''}`);

  if (details.newlySlowPins.length > 0) {
    lines.push(`Newly slow: ${details.newlySlowPins.join(', ')}`);
  }
  if (details.syncTimeoutMs != null) {
    lines.push(`Timeout: ${details.syncTimeoutMs}ms`);
  }
  return lines;
}

export function buildSlowInputAlert(
  edge: AlertEdge,
  details: SlowTimeoutDetails | null,
  connections: MonitorEdgeAlertContext['connections']
): TypedEdgeAlert | null {
  if (!details) return null;
  return {
    kind: 'slow_input_timeout',
    severity: 'warning',
    tooltip: {
      title: `${edge.target} degraded`,
      lines: buildSlowInputTooltipLines(edge, details, connections),
    },
  };
}

export function useSlowInputAlert(
  edge: AlertEdge,
  monitorAlertContext: MonitorEdgeAlertContext | undefined
): TypedEdgeAlert | null {
  const sessionId = monitorAlertContext?.sessionId;
  const targetHandle = edge.targetHandle ?? '';
  const detailsAtom = React.useMemo<SlowInputDetailsAtom>(() => {
    if (!sessionId) return nullSlowInputDetailsAtom;
    return selectAtom(
      nodeStateAtom(nodeKey(sessionId, edge.target)),
      (state) => {
        const details = extractSlowTimeoutDetailsFromNodeState(state);
        if (!details || !details.slowPins.includes(targetHandle)) return null;
        return details;
      },
      deepEqual
    );
  }, [edge.target, sessionId, targetHandle]);
  const details = useAtomValue(detailsAtom);
  return monitorAlertContext
    ? buildSlowInputAlert(edge, details, monitorAlertContext.connections)
    : null;
}

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
  source,
  target,
  sourceHandleId,
  targetHandleId,
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
  const monitorAlertContext = typedData?.monitorAlertContext;
  const dynamicAlert = useSlowInputAlert(
    {
      source,
      sourceHandle: sourceHandleId,
      target,
      targetHandle: targetHandleId,
    },
    monitorAlertContext
  );
  const alert = dynamicAlert ?? typedData?.alert;

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
