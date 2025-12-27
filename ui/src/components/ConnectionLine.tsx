// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { ConnectionLineComponentProps } from '@xyflow/react';
import React from 'react';

const ConnectionLine: React.FC<ConnectionLineComponentProps> = ({
  fromX,
  fromY,
  toX,
  toY,
  connectionStatus,
}) => {
  const centerY = (fromY + toY) / 2;
  const path = `M${fromX},${fromY} C ${fromX},${centerY} ${toX},${centerY} ${toX},${toY}`;

  let color = 'var(--sk-muted)';
  let dash: string | undefined;

  if (connectionStatus === 'valid') {
    color = 'var(--sk-primary)';
  } else if (connectionStatus === 'invalid') {
    color = 'var(--sk-danger)';
    dash = '6 3';
  }

  return (
    <svg style={{ overflow: 'visible' }}>
      <path
        d={path}
        fill="none"
        stroke={color}
        strokeWidth={2.5}
        strokeDasharray={dash}
        strokeLinecap="round"
      />
      <circle cx={fromX} cy={fromY} r={3} fill={color} />
    </svg>
  );
};

export default ConnectionLine;
