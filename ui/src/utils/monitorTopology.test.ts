// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import { buildMonitorTopologyKey, resolveMonitorNodePosition } from './monitorTopology';

describe('buildMonitorTopologyKey', () => {
  it('distinguishes identical topology fingerprints across sessions', () => {
    const topologyFingerprint = JSON.stringify([
      ['source:audio', 'sink:audio'],
      ['source:out>sink:in'],
      ['source'],
      [],
    ]);

    const firstSessionKey = buildMonitorTopologyKey('session-1', topologyFingerprint);
    const secondSessionKey = buildMonitorTopologyKey('session-2', topologyFingerprint);

    expect(firstSessionKey).not.toBe(secondSessionKey);
    expect(JSON.parse(firstSessionKey)[0]).toBe('session-1');
    expect(JSON.parse(secondSessionKey)[0]).toBe('session-2');
  });

  it('preserves live positions for same-session topology rebuilds', () => {
    const previousPositions = new Map([['node', { x: 12, y: 24 }]]);
    const savedPositions = { node: { x: 48, y: 96 } };

    expect(resolveMonitorNodePosition('node', true, previousPositions, savedPositions)).toEqual({
      x: 12,
      y: 24,
    });
  });

  it("uses the target session's saved position after a cross-session rebuild", () => {
    const previousPositions = new Map([['node', { x: 12, y: 24 }]]);
    const savedPositions = { node: { x: 48, y: 96 } };

    expect(resolveMonitorNodePosition('node', false, previousPositions, savedPositions)).toEqual({
      x: 48,
      y: 96,
    });
  });

  it('defaults to the origin when no position is available', () => {
    expect(resolveMonitorNodePosition('node', false, new Map(), {})).toEqual({ x: 0, y: 0 });
  });
});
