// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { NodeState } from '@/types/types';

/** Abbreviated session id for display (first segment before the first dash). */
export const shortSessionId = (sessionId: string): string =>
  sessionId.split('-')[0] || sessionId.slice(0, 8);

export type NodeIssue = {
  nodeId: string;
  summary: string;
};

export function formatIssueDetails(details: unknown): string | null {
  if (details == null) return null;
  try {
    const serialized = JSON.stringify(details);
    if (!serialized || serialized === 'null') return null;
    return serialized.length > 180 ? `${serialized.slice(0, 180)}…` : serialized;
  } catch {
    return null;
  }
}

export function formatIssueSummary(prefix: string, reason: string, details: string | null): string {
  if (!details) return `${prefix}: ${reason}`;
  return `${prefix}: ${reason} (${details})`;
}

export function summarizeNodeIssues(nodeStates: Record<string, NodeState>): NodeIssue[] {
  const issues: NodeIssue[] = [];

  for (const [nodeId, state] of Object.entries(nodeStates)) {
    if (typeof state !== 'object' || state == null) continue;

    if ('Failed' in state) {
      issues.push({ nodeId, summary: `Failed: ${state.Failed.reason}` });
      continue;
    }
    if ('Degraded' in state) {
      const details = formatIssueDetails(state.Degraded.details);
      issues.push({
        nodeId,
        summary: formatIssueSummary('Degraded', state.Degraded.reason, details),
      });
      continue;
    }
    if ('Recovering' in state) {
      const details = formatIssueDetails(state.Recovering.details);
      issues.push({
        nodeId,
        summary: formatIssueSummary('Recovering', state.Recovering.reason, details),
      });
      continue;
    }
    if ('Stopped' in state) {
      issues.push({ nodeId, summary: `Stopped: ${state.Stopped.reason}` });
      continue;
    }
  }

  const priority = (issue: NodeIssue): number => {
    if (issue.summary.startsWith('Failed:')) return 0;
    if (issue.summary.startsWith('Degraded:')) return 1;
    if (issue.summary.startsWith('Recovering:')) return 2;
    if (issue.summary.startsWith('Stopped:')) return 3;
    return 4;
  };

  return issues.sort((a, b) => priority(a) - priority(b) || a.nodeId.localeCompare(b.nodeId));
}
