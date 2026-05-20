// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import type { NodeState } from '@/types/generated/api-types';

import {
  formatIssueDetails,
  formatIssueSummary,
  shortSessionId,
  summarizeNodeIssues,
} from './nodeIssues';

describe('shortSessionId', () => {
  it('returns the first dash-delimited segment when the id contains a dash', () => {
    expect(shortSessionId('abcd1234-5678-90')).toBe('abcd1234');
  });

  it('returns the whole string when there is no dash (the split yields one segment)', () => {
    expect(shortSessionId('abcdefghijklmnop')).toBe('abcdefghijklmnop');
  });

  it('returns the original string when shorter than 8 characters and no dash', () => {
    expect(shortSessionId('short')).toBe('short');
  });

  it('falls back to the first 8 characters when the first dash-segment is empty', () => {
    // First split segment of '-abcdefghij' is '', which is falsy → slice(0, 8).
    expect(shortSessionId('-abcdefghij')).toBe('-abcdefg');
  });

  it('returns empty string for empty input', () => {
    expect(shortSessionId('')).toBe('');
  });
});

describe('formatIssueDetails', () => {
  it('returns null for null details', () => {
    expect(formatIssueDetails(null)).toBeNull();
  });

  it('returns null for undefined details', () => {
    expect(formatIssueDetails(undefined)).toBeNull();
  });

  it('returns null when JSON.stringify yields the literal "null" string', () => {
    // A `toJSON` returning `null` serializes to the string "null"; this exercises
    // the post-stringify guard (not the early `details == null` short-circuit).
    expect(formatIssueDetails({ toJSON: () => null })).toBeNull();
  });

  it('serializes simple objects to JSON', () => {
    expect(formatIssueDetails({ code: 1, msg: 'oops' })).toBe('{"code":1,"msg":"oops"}');
  });

  it('serializes arrays to JSON', () => {
    expect(formatIssueDetails([1, 2, 3])).toBe('[1,2,3]');
  });

  it('truncates output to 180 chars + ellipsis when serialization exceeds 180 chars', () => {
    const big = { s: 'x'.repeat(500) };
    const result = formatIssueDetails(big);
    expect(result).not.toBeNull();
    // 180 chars + the single '…' suffix
    expect(result!.length).toBe(181);
    expect(result!.endsWith('…')).toBe(true);
  });

  it('does not truncate when serialization is exactly 180 chars', () => {
    const exactly = { s: 'x'.repeat(180 - 8) }; // {"s":"..."} envelope is 8 chars
    const result = formatIssueDetails(exactly);
    expect(result).not.toBeNull();
    expect(result!.length).toBe(180);
    expect(result!.endsWith('…')).toBe(false);
  });

  it('returns null when JSON.stringify throws (circular reference)', () => {
    const circular: Record<string, unknown> = {};
    circular.self = circular;
    expect(formatIssueDetails(circular)).toBeNull();
  });
});

describe('formatIssueSummary', () => {
  it('joins prefix and reason with no trailing details when details is null', () => {
    expect(formatIssueSummary('Failed', 'oom', null)).toBe('Failed: oom');
  });

  it('appends parenthesized details when details is present', () => {
    expect(formatIssueSummary('Degraded', 'slow', '{"k":1}')).toBe('Degraded: slow ({"k":1})');
  });

  it('treats empty-string details as "no details"', () => {
    expect(formatIssueSummary('Recovering', 'lag', '')).toBe('Recovering: lag');
  });
});

describe('summarizeNodeIssues', () => {
  it('returns an empty array for an empty input', () => {
    expect(summarizeNodeIssues({})).toEqual([]);
  });

  it('skips non-object states (string variants like "Running")', () => {
    const states: Record<string, NodeState> = {
      node_a: 'Running',
      node_b: 'Ready',
    };
    expect(summarizeNodeIssues(states)).toEqual([]);
  });

  it('reports Failed states with the bare "Failed: <reason>" form (no details)', () => {
    const states: Record<string, NodeState> = {
      n1: { Failed: { reason: 'panic' } },
    };
    expect(summarizeNodeIssues(states)).toEqual([{ nodeId: 'n1', summary: 'Failed: panic' }]);
  });

  it('reports Degraded states including parenthesized details when present', () => {
    const states: Record<string, NodeState> = {
      n1: { Degraded: { reason: 'slow', details: { ms: 500 } } },
    };
    expect(summarizeNodeIssues(states)).toEqual([
      { nodeId: 'n1', summary: 'Degraded: slow ({"ms":500})' },
    ]);
  });

  it('omits details for Degraded when the details value is null', () => {
    const states: Record<string, NodeState> = {
      n1: { Degraded: { reason: 'slow', details: null } },
    };
    expect(summarizeNodeIssues(states)).toEqual([{ nodeId: 'n1', summary: 'Degraded: slow' }]);
  });

  it('reports Recovering states with formatted details', () => {
    const states: Record<string, NodeState> = {
      n1: { Recovering: { reason: 'reconnecting', details: { attempt: 3 } } },
    };
    expect(summarizeNodeIssues(states)).toEqual([
      { nodeId: 'n1', summary: 'Recovering: reconnecting ({"attempt":3})' },
    ]);
  });

  it('reports Stopped states with the StopReason as the reason', () => {
    const states: Record<string, NodeState> = {
      n1: { Stopped: { reason: 'completed' } },
    };
    expect(summarizeNodeIssues(states)).toEqual([{ nodeId: 'n1', summary: 'Stopped: completed' }]);
  });

  it('orders issues by severity (Failed < Degraded < Recovering < Stopped), then by nodeId', () => {
    const states: Record<string, NodeState> = {
      z_stop: { Stopped: { reason: 'shutdown' } },
      a_recov: { Recovering: { reason: 'r1', details: null } },
      m_fail: { Failed: { reason: 'boom' } },
      b_degr: { Degraded: { reason: 'd1', details: null } },
      c_fail: { Failed: { reason: 'boom2' } },
    };
    const result = summarizeNodeIssues(states);
    expect(result.map((i) => i.nodeId)).toEqual([
      'c_fail',
      'm_fail',
      'b_degr',
      'a_recov',
      'z_stop',
    ]);
  });

  it('skips null state entries without throwing', () => {
    const states = {
      live: 'Running' as NodeState,
      // The `state: NodeState | null` is allowed in API responses.
      dead: null as unknown as NodeState,
    };
    expect(summarizeNodeIssues(states)).toEqual([]);
  });
});
