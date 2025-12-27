// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { Handle, Position } from '@xyflow/react';
import React from 'react';

import { SKTooltip } from '@/components/Tooltip';
import type { PinCardinality } from '@/types/generated/api-types';
import type { PacketType } from '@/types/types';
import {
  formatPacketType,
  formatPinCardinality,
  getPacketTypeColor,
  getPinCardinalityDescription,
  getPinCardinalityIcon,
} from '@/utils/packetTypes';

export const HANDLE_STYLE = {
  size: 12,
  radius: 2,
  border: '2px solid var(--sk-border-strong)',
  boxShadow: '0 0 0 2px var(--sk-panel-bg)',
} as const;

type PinHandleProps = {
  id: string;
  name: string;
  packetType: PacketType;
  cardinality: PinCardinality;
  type: 'source' | 'target';
  position: Position;
  leftPercent?: number; // for top/bottom
  topPercent?: number; // for left/right
};

export const PinHandle: React.FC<PinHandleProps> = ({
  id,
  name,
  packetType,
  cardinality,
  type,
  position,
  leftPercent,
  topPercent,
}) => {
  const color = getPacketTypeColor(packetType);
  const isInput = type === 'target';
  const cardinalityIcon = getPinCardinalityIcon(cardinality);
  const cardinalityText = formatPinCardinality(cardinality);
  const cardinalityDescription = getPinCardinalityDescription(cardinality, isInput);

  const halfSize = HANDLE_STYLE.size / 2;
  const borderWidth = 2; // Account for the node's border width

  const style: React.CSSProperties =
    position === Position.Top || position === Position.Bottom
      ? {
          left: `${leftPercent ?? 50}%`,
          transform: 'translateX(-50%)',
          top: position === Position.Top ? -(halfSize + borderWidth) : undefined,
          bottom: position === Position.Bottom ? -(halfSize + borderWidth) : undefined,
          width: HANDLE_STYLE.size,
          height: HANDLE_STYLE.size,
          background: color,
          borderRadius: HANDLE_STYLE.radius,
          border: HANDLE_STYLE.border as string,
          boxShadow: HANDLE_STYLE.boxShadow,
        }
      : {
          top: `${topPercent ?? 50}%`,
          transform: 'translateY(-50%)',
          left: position === Position.Left ? -(halfSize + borderWidth) : undefined,
          right: position === Position.Right ? -(halfSize + borderWidth) : undefined,
          width: HANDLE_STYLE.size,
          height: HANDLE_STYLE.size,
          background: color,
          borderRadius: HANDLE_STYLE.radius,
          border: HANDLE_STYLE.border as string,
          boxShadow: HANDLE_STYLE.boxShadow,
        };

  return (
    <SKTooltip
      content={
        <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <span
              style={{
                width: 10,
                height: 10,
                background: color,
                borderRadius: 4,
                border: '1px solid var(--sk-border-strong)',
                display: 'inline-block',
              }}
            />
            <div>
              <div className="code-font" style={{ fontWeight: 600 }}>
                {name}
              </div>
              <div style={{ fontSize: 12, color: 'var(--sk-text-muted)' }}>
                {formatPacketType(packetType)}
              </div>
            </div>
          </div>
          <div
            style={{
              fontSize: 11,
              color: 'var(--sk-text-muted)',
              paddingTop: 4,
              borderTop: '1px solid var(--sk-border-weak)',
            }}
          >
            <div style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
              <span style={{ fontSize: 14 }}>{cardinalityIcon}</span>
              <span>{cardinalityText}</span>
            </div>
            <div style={{ fontSize: 10, marginTop: 2 }}>{cardinalityDescription}</div>
          </div>
        </div>
      }
    >
      <Handle id={id} type={type} position={position} style={style} />
    </SKTooltip>
  );
};
