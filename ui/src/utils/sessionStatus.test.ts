// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import type { NodeState } from '@/types/types';

import {
  computeSessionStatus,
  getSessionStatusColor,
  getSessionStatusLabel,
  type SessionStatus,
} from './sessionStatus';

const failed: NodeState = { Failed: { reason: 'boom' } };
const stopped: NodeState = { Stopped: { reason: 'completed' } };
const degraded: NodeState = { Degraded: { reason: 'slow_input_timeout', details: null } };
const recovering: NodeState = { Recovering: { reason: 'restart', details: null } };

describe('computeSessionStatus', () => {
  it('returns "unknown" for an empty record', () => {
    expect(computeSessionStatus({})).toBe('unknown');
  });

  it('returns "failed" when any state is Failed (highest precedence)', () => {
    expect(
      computeSessionStatus({
        a: 'Running',
        b: degraded,
        c: stopped,
        d: failed,
      })
    ).toBe('failed');
  });

  it('returns "stopped" when any state is Stopped and none Failed', () => {
    expect(
      computeSessionStatus({
        a: 'Running',
        b: degraded,
        c: recovering,
        d: stopped,
      })
    ).toBe('stopped');
  });

  it('returns "degraded" when Degraded is present and no Failed/Stopped', () => {
    expect(
      computeSessionStatus({
        a: 'Running',
        b: recovering,
        c: degraded,
      })
    ).toBe('degraded');
  });

  it('returns "recovering" when only Recovering is the worst state', () => {
    expect(
      computeSessionStatus({
        a: 'Running',
        b: recovering,
      })
    ).toBe('recovering');
  });

  it('returns "initializing" when any state is Creating or Initializing', () => {
    expect(computeSessionStatus({ a: 'Running', b: 'Initializing' })).toBe('initializing');
    expect(computeSessionStatus({ a: 'Creating', b: 'Creating' })).toBe('initializing');
    expect(computeSessionStatus({ a: 'Running', b: 'Creating' })).toBe('initializing');
  });

  it('returns "running" only when every state is exactly Running', () => {
    expect(computeSessionStatus({ a: 'Running' })).toBe('running');
    expect(computeSessionStatus({ a: 'Running', b: 'Running', c: 'Running' })).toBe('running');
  });

  it('returns "unknown" when Running is mixed with an unrecognized state', () => {
    expect(computeSessionStatus({ a: 'Running', b: 'Ready' })).toBe('unknown');
  });

  it('prioritises Failed over Stopped, Degraded, and Recovering', () => {
    expect(
      computeSessionStatus({
        a: failed,
        b: stopped,
        c: degraded,
        d: recovering,
      })
    ).toBe('failed');
  });

  it('prioritises Stopped over Degraded and Recovering', () => {
    expect(
      computeSessionStatus({
        a: stopped,
        b: degraded,
        c: recovering,
      })
    ).toBe('stopped');
  });

  it('prioritises Degraded over Recovering and Initializing', () => {
    expect(
      computeSessionStatus({
        a: degraded,
        b: recovering,
        c: 'Initializing',
      })
    ).toBe('degraded');
  });

  it('prioritises Recovering over Initializing', () => {
    expect(
      computeSessionStatus({
        a: recovering,
        b: 'Initializing',
      })
    ).toBe('recovering');
  });
});

const ALL_STATUSES: SessionStatus[] = [
  'running',
  'initializing',
  'degraded',
  'recovering',
  'failed',
  'stopped',
  'unknown',
];

describe('getSessionStatusColor', () => {
  it.each(ALL_STATUSES)('returns a non-empty CSS value for %s', (status) => {
    const color = getSessionStatusColor(status);
    expect(typeof color).toBe('string');
    expect(color.length).toBeGreaterThan(0);
    expect(color).toMatch(/^var\(--sk-[a-z-]+\)$/);
  });

  it('maps every status to its documented custom property', () => {
    expect(getSessionStatusColor('running')).toBe('var(--sk-status-running)');
    expect(getSessionStatusColor('initializing')).toBe('var(--sk-status-initializing)');
    expect(getSessionStatusColor('degraded')).toBe('var(--sk-status-degraded)');
    expect(getSessionStatusColor('recovering')).toBe('var(--sk-status-recovering)');
    expect(getSessionStatusColor('failed')).toBe('var(--sk-status-failed)');
    expect(getSessionStatusColor('stopped')).toBe('var(--sk-status-stopped)');
    expect(getSessionStatusColor('unknown')).toBe('var(--sk-text-muted)');
  });

  it('returns distinct colors for distinct statuses (except unknown which reuses muted text)', () => {
    const distinctStatuses: SessionStatus[] = [
      'running',
      'initializing',
      'degraded',
      'recovering',
      'failed',
      'stopped',
    ];
    const colors = distinctStatuses.map(getSessionStatusColor);
    expect(new Set(colors).size).toBe(distinctStatuses.length);
  });
});

describe('getSessionStatusLabel', () => {
  it.each(ALL_STATUSES)('returns a non-empty human-readable label for %s', (status) => {
    const label = getSessionStatusLabel(status);
    expect(typeof label).toBe('string');
    expect(label.length).toBeGreaterThan(0);
  });

  it('maps each status to a capitalised label matching the status name', () => {
    expect(getSessionStatusLabel('running')).toBe('Running');
    expect(getSessionStatusLabel('initializing')).toBe('Initializing');
    expect(getSessionStatusLabel('degraded')).toBe('Degraded');
    expect(getSessionStatusLabel('recovering')).toBe('Recovering');
    expect(getSessionStatusLabel('failed')).toBe('Failed');
    expect(getSessionStatusLabel('stopped')).toBe('Stopped');
    expect(getSessionStatusLabel('unknown')).toBe('Unknown');
  });

  it('returns a distinct label per status', () => {
    const labels = ALL_STATUSES.map(getSessionStatusLabel);
    expect(new Set(labels).size).toBe(ALL_STATUSES.length);
  });
});
