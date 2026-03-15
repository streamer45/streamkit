// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Node-state legend overlay for the Monitor View canvas.
 * Memoized to prevent re-renders during node drag.
 */

import React from 'react';

import {
  LegendContainer,
  LegendTitle,
  LegendItem,
  LegendDot,
} from '@/components/monitor/MonitorView.styles';

export const Legend = React.memo(() => (
  <LegendContainer>
    <LegendTitle>Node States</LegendTitle>
    <LegendItem>
      <LegendDot color="var(--sk-status-initializing)" />
      <span>Initializing</span>
    </LegendItem>
    <LegendItem>
      <LegendDot color="var(--sk-status-running)" />
      <span>Running</span>
    </LegendItem>
    <LegendItem>
      <LegendDot color="var(--sk-status-recovering)" />
      <span>Recovering</span>
    </LegendItem>
    <LegendItem>
      <LegendDot color="var(--sk-status-degraded)" />
      <span>Degraded</span>
    </LegendItem>
    <LegendItem>
      <LegendDot color="var(--sk-status-failed)" />
      <span>Failed</span>
    </LegendItem>
    <LegendItem>
      <LegendDot color="var(--sk-status-stopped)" />
      <span>Stopped</span>
    </LegendItem>
  </LegendContainer>
));
