// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, it, expect, beforeEach } from 'vitest';

import type { Pipeline, NodeState, NodeStats } from '@/types/types';

import {
  sessionStore,
  nodeStateAtom,
  nodeStatsAtom,
  nodeViewDataAtom,
  nodeParamsAtom,
  sessionConnectedAtom,
  nodeKey,
  batchWriteNodeStates,
  batchWriteNodeStats,
  seedPipelineAtoms,
  clearSessionAtoms,
  writeSessionConnected,
} from './sessionAtoms';

// Helper to reset all atoms created during a test.
function resetAtoms(): void {
  for (const key of [...nodeStateAtom.getParams()]) {
    sessionStore.set(nodeStateAtom(key), null);
    nodeStateAtom.remove(key);
  }
  for (const key of [...nodeStatsAtom.getParams()]) {
    sessionStore.set(nodeStatsAtom(key), null);
    nodeStatsAtom.remove(key);
  }
  for (const key of [...nodeViewDataAtom.getParams()]) {
    sessionStore.set(nodeViewDataAtom(key), undefined);
    nodeViewDataAtom.remove(key);
  }
  for (const key of [...nodeParamsAtom.getParams()]) {
    sessionStore.set(nodeParamsAtom(key), {});
    nodeParamsAtom.remove(key);
  }
  for (const key of [...sessionConnectedAtom.getParams()]) {
    sessionStore.set(sessionConnectedAtom(key), false);
    sessionConnectedAtom.remove(key);
  }
}

describe('sessionAtoms', () => {
  beforeEach(resetAtoms);

  describe('batchWriteNodeStates', () => {
    it('should write per-node state atoms', () => {
      const updates = new Map<string, Record<string, NodeState>>();
      updates.set('s1', { 'node-a': 'Running', 'node-b': 'Initializing' });

      batchWriteNodeStates(updates);

      expect(sessionStore.get(nodeStateAtom(nodeKey('s1', 'node-a')))).toBe('Running');
      expect(sessionStore.get(nodeStateAtom(nodeKey('s1', 'node-b')))).toBe('Initializing');
    });

    it('should handle multiple sessions', () => {
      const updates = new Map<string, Record<string, NodeState>>();
      updates.set('s1', { n1: 'Running' });
      updates.set('s2', { n2: 'Initializing' });

      batchWriteNodeStates(updates);

      expect(sessionStore.get(nodeStateAtom(nodeKey('s1', 'n1')))).toBe('Running');
      expect(sessionStore.get(nodeStateAtom(nodeKey('s2', 'n2')))).toBe('Initializing');
      // Cross-session isolation
      expect(sessionStore.get(nodeStateAtom(nodeKey('s1', 'n2')))).toBeNull();
    });
  });

  describe('batchWriteNodeStats', () => {
    it('should write per-node stats atoms', () => {
      const stats: NodeStats = {
        received: BigInt(100),
        sent: BigInt(95),
        discarded: BigInt(5),
        errored: BigInt(0),
        duration_secs: 10.0,
      };
      const updates = new Map<string, Record<string, NodeStats>>();
      updates.set('s1', { n1: stats });

      batchWriteNodeStats(updates);

      expect(sessionStore.get(nodeStatsAtom(nodeKey('s1', 'n1')))).toEqual(stats);
    });
  });

  describe('seedPipelineAtoms', () => {
    it('should seed node state atoms from pipeline', () => {
      const pipeline: Pipeline = {
        name: null,
        description: null,
        mode: 'dynamic',
        client: null,
        nodes: {
          gain: { kind: 'audio::gain', params: { gain: 1.0 }, state: 'Running' },
          mixer: { kind: 'audio::mixer', params: {}, state: 'Initializing' },
        },
        connections: [],
      };

      seedPipelineAtoms('s1', pipeline);

      expect(sessionStore.get(nodeStateAtom(nodeKey('s1', 'gain')))).toBe('Running');
      expect(sessionStore.get(nodeStateAtom(nodeKey('s1', 'mixer')))).toBe('Initializing');
    });

    it('should seed view data atoms from pipeline', () => {
      const pipeline: Pipeline = {
        name: null,
        description: null,
        mode: 'dynamic',
        client: null,
        nodes: {
          comp: { kind: 'video::compositor', params: {}, state: null },
        },
        connections: [],
        view_data: {
          comp: { layers: { in_0: { x: 0, y: 0 } } },
        },
      };

      seedPipelineAtoms('s1', pipeline);

      expect(sessionStore.get(nodeViewDataAtom(nodeKey('s1', 'comp')))).toEqual({
        layers: { in_0: { x: 0, y: 0 } },
      });
    });

    it('should skip nodes without state', () => {
      const pipeline: Pipeline = {
        name: null,
        description: null,
        mode: 'dynamic',
        client: null,
        nodes: {
          gain: { kind: 'audio::gain', params: {}, state: null },
        },
        connections: [],
      };

      seedPipelineAtoms('s1', pipeline);

      expect(sessionStore.get(nodeStateAtom(nodeKey('s1', 'gain')))).toBeNull();
    });
  });

  describe('clearSessionAtoms', () => {
    it('should clear all atoms for a session', () => {
      // Seed some data
      const updates = new Map<string, Record<string, NodeState>>();
      updates.set('s1', { n1: 'Running', n2: 'Initializing' });
      batchWriteNodeStates(updates);
      writeSessionConnected('s1', true);

      // Verify data exists
      expect(sessionStore.get(nodeStateAtom(nodeKey('s1', 'n1')))).toBe('Running');
      expect(sessionStore.get(sessionConnectedAtom('s1'))).toBe(true);

      // Clear
      clearSessionAtoms('s1');

      // Verify cleared
      expect(sessionStore.get(nodeStateAtom(nodeKey('s1', 'n1')))).toBeNull();
      expect(sessionStore.get(sessionConnectedAtom('s1'))).toBe(false);
    });

    it('should not affect atoms from other sessions', () => {
      const updates = new Map<string, Record<string, NodeState>>();
      updates.set('s1', { n1: 'Running' });
      updates.set('s2', { n2: 'Running' });
      batchWriteNodeStates(updates);

      clearSessionAtoms('s1');

      // s1 cleared
      expect(sessionStore.get(nodeStateAtom(nodeKey('s1', 'n1')))).toBeNull();
      // s2 untouched
      expect(sessionStore.get(nodeStateAtom(nodeKey('s2', 'n2')))).toBe('Running');
    });
  });
});
