// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { NodeState } from '@/types/types';

export type SessionStatus =
  'running' | 'initializing' | 'degraded' | 'recovering' | 'failed' | 'stopped' | 'unknown';

export function computeSessionStatus(nodeStates: Record<string, NodeState>): SessionStatus {
  const states = Object.values(nodeStates);

  if (states.length === 0) {
    return 'unknown';
  }

  if (states.some((state) => typeof state === 'object' && 'Failed' in state)) {
    return 'failed';
  }

  if (states.some((state) => typeof state === 'object' && 'Stopped' in state)) {
    return 'stopped';
  }

  if (states.some((state) => typeof state === 'object' && 'Degraded' in state)) {
    return 'degraded';
  }

  if (states.some((state) => typeof state === 'object' && 'Recovering' in state)) {
    return 'recovering';
  }

  if (states.some((state) => state === 'Creating' || state === 'Initializing')) {
    return 'initializing';
  }

  if (states.every((state) => state === 'Running')) {
    return 'running';
  }

  return 'unknown';
}

export function getSessionStatusColor(status: SessionStatus): string {
  switch (status) {
    case 'running':
      return 'var(--sk-status-running)';
    case 'initializing':
      return 'var(--sk-status-initializing)';
    case 'degraded':
      return 'var(--sk-status-degraded)';
    case 'recovering':
      return 'var(--sk-status-recovering)';
    case 'failed':
      return 'var(--sk-status-failed)';
    case 'stopped':
      return 'var(--sk-status-stopped)';
    case 'unknown':
      return 'var(--sk-text-muted)';
  }
}

export function getSessionStatusLabel(status: SessionStatus): string {
  switch (status) {
    case 'running':
      return 'Running';
    case 'initializing':
      return 'Initializing';
    case 'degraded':
      return 'Degraded';
    case 'recovering':
      return 'Recovering';
    case 'failed':
      return 'Failed';
    case 'stopped':
      return 'Stopped';
    case 'unknown':
      return 'Unknown';
  }
}
