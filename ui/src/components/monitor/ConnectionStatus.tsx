// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import React from 'react';

import {
  ConnectionStatusContainer,
  ConnectionStatusDot,
} from '@/components/monitor/MonitorView.styles';

// Memoized ConnectionStatus component
export const ConnectionStatus = React.memo(({ connected }: { connected: boolean }) => (
  <ConnectionStatusContainer connected={connected}>
    <ConnectionStatusDot connected={connected} />
    {connected ? 'Connected' : 'Disconnected'}
  </ConnectionStatusContainer>
));
