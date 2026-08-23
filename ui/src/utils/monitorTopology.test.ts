// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import { buildMonitorTopologyKey } from './monitorTopology';

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
});
