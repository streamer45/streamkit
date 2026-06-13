// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import {
  ConnectionStatusContainer,
  ConnectionStatusDot,
} from '@/components/monitor/MonitorView.styles';

export const ConnectionStatus = ({ connected }: { connected: boolean }) => (
  <ConnectionStatusContainer connected={connected}>
    <ConnectionStatusDot connected={connected} />
    {connected ? 'Connected' : 'Disconnected'}
  </ConnectionStatusContainer>
);
