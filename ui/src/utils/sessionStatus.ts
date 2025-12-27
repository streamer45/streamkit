// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { NodeState } from '@/types/types';

/**
 * Aggregated session status based on node states
 */
export type SessionStatus =
  | 'running'
  | 'initializing'
  | 'degraded'
  | 'recovering'
  | 'failed'
  | 'stopped'
  | 'unknown';

/**
 * Compute aggregated session status from all node states
 * Priority order (highest to lowest):
 * 1. Failed - Any node failed
 * 2. Stopped - Any node stopped
 * 3. Degraded - Any node degraded
 * 4. Recovering - Any node recovering
 * 5. Initializing - Any node initializing (and none in worse states)
 * 6. Running - All nodes running
 * 7. Unknown - No nodes or unable to determine
 */
export function computeSessionStatus(nodeStates: Record<string, NodeState>): SessionStatus {
  const states = Object.values(nodeStates);

  if (states.length === 0) {
    return 'unknown';
  }

  // Check for failure (highest priority)
  if (states.some((state) => typeof state === 'object' && 'Failed' in state)) {
    return 'failed';
  }

  // Check for stopped
  if (states.some((state) => typeof state === 'object' && 'Stopped' in state)) {
    return 'stopped';
  }

  // Check for degraded
  if (states.some((state) => typeof state === 'object' && 'Degraded' in state)) {
    return 'degraded';
  }

  // Check for recovering
  if (states.some((state) => typeof state === 'object' && 'Recovering' in state)) {
    return 'recovering';
  }

  // Check for initializing
  if (states.some((state) => state === 'Initializing')) {
    return 'initializing';
  }

  // All running
  if (states.every((state) => state === 'Running')) {
    return 'running';
  }

  return 'unknown';
}

/**
 * Get color for session status
 */
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

/**
 * Get label for session status
 */
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
