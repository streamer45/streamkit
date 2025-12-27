// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import { Handle, Position } from '@xyflow/react';
import React from 'react';

import { SKTooltip } from '@/components/Tooltip';
import type { PacketType } from '@/types/types';
import { getPacketTypeColor } from '@/utils/packetTypes';

import { HANDLE_STYLE } from './PinHandle';

const TooltipContent = styled.div`
  font-size: 11px;
  color: var(--sk-text-muted);
`;

const GUTTER_PCT = 8; // keep pins away from the corners (same as PinRow)

function percent(i: number, total: number) {
  if (total <= 1) return 50;
  const usable = 100 - 2 * GUTTER_PCT;
  return GUTTER_PCT + (i * usable) / (total - 1);
}

// Helper: Get ghost pin style for horizontal sides (top/bottom)
function getHorizontalGhostStyle(
  side: 'top' | 'bottom',
  positionPercent: number,
  halfSize: number,
  borderWidth: number,
  bgColor: string
): React.CSSProperties {
  return {
    left: `${positionPercent}%`,
    transform: 'translateX(-50%)',
    top: side === 'top' ? -(halfSize + borderWidth) : undefined,
    bottom: side === 'bottom' ? -(halfSize + borderWidth) : undefined,
    width: HANDLE_STYLE.size,
    height: HANDLE_STYLE.size,
    background: `repeating-linear-gradient(
      45deg,
      ${bgColor}70,
      ${bgColor}70 2px,
      ${bgColor}30 2px,
      ${bgColor}30 4px
    )`,
    borderRadius: HANDLE_STYLE.radius,
    border: '2px dashed var(--sk-border-strong)',
    boxShadow: `0 0 0 2px var(--sk-panel-bg), ${HANDLE_STYLE.boxShadow}`,
    opacity: 0.85,
  };
}

// Helper: Get ghost pin style for vertical sides (left/right)
function getVerticalGhostStyle(
  side: 'left' | 'right',
  positionPercent: number,
  halfSize: number,
  borderWidth: number,
  bgColor: string
): React.CSSProperties {
  return {
    top: `${positionPercent}%`,
    transform: 'translateY(-50%)',
    left: side === 'left' ? -(halfSize + borderWidth) : undefined,
    right: side === 'right' ? -(halfSize + borderWidth) : undefined,
    width: HANDLE_STYLE.size,
    height: HANDLE_STYLE.size,
    background: `repeating-linear-gradient(
      45deg,
      ${bgColor}70,
      ${bgColor}70 2px,
      ${bgColor}30 2px,
      ${bgColor}30 4px
    )`,
    borderRadius: HANDLE_STYLE.radius,
    border: '2px dashed var(--sk-border-strong)',
    boxShadow: `0 0 0 2px var(--sk-panel-bg), ${HANDLE_STYLE.boxShadow}`,
    opacity: 0.85,
  };
}

type PlaceholderPinRowProps = {
  side: 'top' | 'bottom' | 'left' | 'right';
  isInput: boolean;
  message?: string;
  packetType?: PacketType; // Type hint for the ghost pin
  pinIndex?: number; // Index of this ghost pin in the row
  totalPins?: number; // Total pins including real + ghost
};

export const PlaceholderPinRow: React.FC<PlaceholderPinRowProps> = ({
  side,
  isInput,
  message,
  packetType,
  pinIndex = 0,
  totalPins = 1,
}) => {
  const position =
    side === 'top'
      ? Position.Top
      : side === 'bottom'
        ? Position.Bottom
        : side === 'left'
          ? Position.Left
          : Position.Right;

  const halfSize = HANDLE_STYLE.size / 2;
  const borderWidth = 2;

  // Calculate position percentage based on index and total
  const positionPercent = percent(pinIndex, totalPins);

  // Get color from packet type if provided
  const bgColor = packetType ? getPacketTypeColor(packetType) : 'var(--sk-border)';

  // Ghost pin style - more distinct visual appearance
  // Uses a striped pattern and lighter appearance to differentiate from real pins
  // Positioned using percentage like real pins for even spacing
  const style: React.CSSProperties =
    side === 'top' || side === 'bottom'
      ? getHorizontalGhostStyle(side, positionPercent, halfSize, borderWidth, bgColor)
      : getVerticalGhostStyle(side, positionPercent, halfSize, borderWidth, bgColor);

  const defaultMessage = isInput
    ? 'Drop connection to create new input pin'
    : 'Outputs will be created dynamically';

  return (
    <SKTooltip content={<TooltipContent>{message || defaultMessage}</TooltipContent>}>
      <Handle
        id={`__ghost__${isInput ? 'in' : 'out'}`}
        type={isInput ? 'target' : 'source'}
        position={position}
        style={style}
      />
    </SKTooltip>
  );
};
