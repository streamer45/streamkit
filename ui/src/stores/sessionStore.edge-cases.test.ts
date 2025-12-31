// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Edge case tests for sessionStore
 * Split from main test file to comply with max-lines rule
 */

import { describe, it, expect, beforeEach } from 'vitest';

import type { Pipeline, NodeState } from '@/types/types';

import { useSessionStore } from './sessionStore';

describe('sessionStore edge cases', () => {
  const TEST_SESSION_ID = 'test-session-1';

  beforeEach(() => {
    useSessionStore.setState({ sessions: new Map() });
  });

  describe('Multi-Session Edge Cases', () => {
    it('should handle concurrent updates to different sessions', () => {
      const session1 = 'session-1';
      const session2 = 'session-2';

      // Update both sessions concurrently
      useSessionStore.getState().updateNodeState(session1, 'node-1', 'Running');
      useSessionStore.getState().updateNodeState(session2, 'node-2', 'Initializing');

      const s1 = useSessionStore.getState().getSession(session1);
      const s2 = useSessionStore.getState().getSession(session2);

      expect(s1?.nodeStates['node-1']).toBe('Running');
      expect(s2?.nodeStates['node-2']).toBe('Initializing');
      // Ensure sessions are isolated
      expect(s1?.nodeStates['node-2']).toBeUndefined();
      expect(s2?.nodeStates['node-1']).toBeUndefined();
    });

    it('should maintain session isolation when updating pipelines', () => {
      const session1 = 'session-1';
      const session2 = 'session-2';
      const pipeline1: Pipeline = {
        name: null,
        description: null,
        mode: 'dynamic',
        nodes: { 'node-1': { kind: 'core::passthrough', params: {}, state: 'Initializing' } },
        connections: [],
      };
      const pipeline2: Pipeline = {
        name: null,
        description: null,
        mode: 'dynamic',
        nodes: { 'node-2': { kind: 'core::gain', params: { gain: 1.0 }, state: 'Initializing' } },
        connections: [],
      };

      useSessionStore.getState().setPipeline(session1, pipeline1);
      useSessionStore.getState().setPipeline(session2, pipeline2);

      const s1 = useSessionStore.getState().getSession(session1);
      const s2 = useSessionStore.getState().getSession(session2);

      expect(s1?.pipeline?.nodes['node-1']).toBeDefined();
      expect(s1?.pipeline?.nodes['node-2']).toBeUndefined();
      expect(s2?.pipeline?.nodes['node-2']).toBeDefined();
      expect(s2?.pipeline?.nodes['node-1']).toBeUndefined();
    });

    it('should handle rapid state updates to the same node', () => {
      const nodeId = 'node-1';
      const states: NodeState[] = [
        'Initializing',
        'Running',
        { Degraded: { reason: 'test degradation', details: null } },
        'Running',
      ];

      states.forEach((state) => {
        useSessionStore.getState().updateNodeState(TEST_SESSION_ID, nodeId, state);
      });

      const session = useSessionStore.getState().getSession(TEST_SESSION_ID);
      expect(session?.nodeStates[nodeId]).toBe('Running'); // Last update wins
    });
  });

  describe('updateNodeParams - Type Guards', () => {
    beforeEach(() => {
      const pipeline: Pipeline = {
        name: null,
        description: null,
        mode: 'dynamic',
        nodes: {
          'node-1': {
            kind: 'core::passthrough',
            params: { gain: 1.0, threshold: 0.5 },
            state: 'Initializing',
          },
          'node-2': { kind: 'core::script', params: 'some string', state: 'Initializing' }, // Non-object params
          'node-3': { kind: 'core::gain', params: null, state: 'Initializing' }, // Null params
          'node-4': { kind: 'core::mixer', params: ['item1', 'item2'], state: 'Initializing' }, // Array params
        },
        connections: [],
      };
      useSessionStore.getState().setPipeline(TEST_SESSION_ID, pipeline);
    });

    it('should merge params when existing params is an object', () => {
      useSessionStore.getState().updateNodeParams(TEST_SESSION_ID, 'node-1', { gain: 2.0 });

      const session = useSessionStore.getState().getSession(TEST_SESSION_ID);
      const params = session?.pipeline?.nodes['node-1'].params as Record<string, unknown>;

      // Should merge: keep threshold, update gain
      expect(params.gain).toBe(2.0);
      expect(params.threshold).toBe(0.5);
    });

    it('should replace params when existing params is a string', () => {
      useSessionStore.getState().updateNodeParams(TEST_SESSION_ID, 'node-2', { newParam: 'value' });

      const session = useSessionStore.getState().getSession(TEST_SESSION_ID);
      const params = session?.pipeline?.nodes['node-2'].params;

      // Should replace entirely (not merge with string)
      expect(params).toEqual({ newParam: 'value' });
    });

    it('should replace params when existing params is null', () => {
      useSessionStore.getState().updateNodeParams(TEST_SESSION_ID, 'node-3', { gain: 1.5 });

      const session = useSessionStore.getState().getSession(TEST_SESSION_ID);
      const params = session?.pipeline?.nodes['node-3'].params;

      expect(params).toEqual({ gain: 1.5 });
    });

    it('should replace params when existing params is an array', () => {
      useSessionStore.getState().updateNodeParams(TEST_SESSION_ID, 'node-4', { count: 5 });

      const session = useSessionStore.getState().getSession(TEST_SESSION_ID);
      const params = session?.pipeline?.nodes['node-4'].params;

      // Arrays are not merged, should replace entirely
      expect(params).toEqual({ count: 5 });
    });

    it('should no-op when session does not exist', () => {
      const beforeState = useSessionStore.getState().sessions;

      useSessionStore.getState().updateNodeParams('non-existent-session', 'node-1', { value: 1 });

      const afterState = useSessionStore.getState().sessions;

      // State should be unchanged
      expect(afterState).toBe(beforeState);
    });

    it('should no-op when session has no pipeline', () => {
      // Create session without pipeline
      useSessionStore.getState().updateNodeState('empty-session', 'node-1', 'Running');

      useSessionStore.getState().updateNodeParams('empty-session', 'node-1', { value: 1 });

      const afterSessions = useSessionStore.getState().sessions;
      const session = afterSessions.get('empty-session');

      // Session exists but has no pipeline
      expect(session?.pipeline).toBeNull();
    });
  });

  describe('Operations on Non-Existent Sessions', () => {
    it('should no-op when adding node to non-existent session', () => {
      const beforeState = useSessionStore.getState().sessions;

      useSessionStore.getState().addNode('non-existent', 'node-1', {
        kind: 'core::passthrough',
        params: {},
      });

      const afterState = useSessionStore.getState().sessions;
      expect(afterState).toBe(beforeState); // No change
    });

    it('should no-op when removing node from non-existent session', () => {
      const beforeState = useSessionStore.getState().sessions;

      useSessionStore.getState().removeNode('non-existent', 'node-1');

      const afterState = useSessionStore.getState().sessions;
      expect(afterState).toBe(beforeState); // No change
    });

    it('should no-op when adding connection to non-existent session', () => {
      const beforeState = useSessionStore.getState().sessions;

      useSessionStore.getState().addConnection('non-existent', {
        from_node: 'node-1',
        from_pin: 'output',
        to_node: 'node-2',
        to_pin: 'input',
      });

      const afterState = useSessionStore.getState().sessions;
      expect(afterState).toBe(beforeState); // No change
    });

    it('should no-op when removing connection from non-existent session', () => {
      const beforeState = useSessionStore.getState().sessions;

      useSessionStore.getState().removeConnection('non-existent', {
        from_node: 'node-1',
        from_pin: 'output',
        to_node: 'node-2',
        to_pin: 'input',
      });

      const afterState = useSessionStore.getState().sessions;
      expect(afterState).toBe(beforeState); // No change
    });
  });

  describe('Pipeline Updates with Missing Nodes', () => {
    beforeEach(() => {
      const pipeline: Pipeline = {
        name: null,
        description: null,
        mode: 'dynamic',
        nodes: {
          'node-1': { kind: 'core::passthrough', params: {}, state: 'Initializing' },
        },
        connections: [],
      };
      useSessionStore.getState().setPipeline(TEST_SESSION_ID, pipeline);
    });

    it('should handle updateNodeParams on missing node gracefully', () => {
      useSessionStore.getState().updateNodeParams(TEST_SESSION_ID, 'non-existent-node', {
        value: 1,
      });

      const session = useSessionStore.getState().getSession(TEST_SESSION_ID);

      // Should add the node params (creates new node entry)
      expect(session?.pipeline?.nodes['non-existent-node']).toBeDefined();
    });

    it('should remove connections when removing a node', () => {
      // Add a second node and connection
      useSessionStore.getState().addNode(TEST_SESSION_ID, 'node-2', {
        kind: 'core::gain',
        params: {},
      });
      useSessionStore.getState().addConnection(TEST_SESSION_ID, {
        from_node: 'node-1',
        from_pin: 'output',
        to_node: 'node-2',
        to_pin: 'input',
      });

      const beforeSession = useSessionStore.getState().getSession(TEST_SESSION_ID);
      expect(beforeSession?.pipeline?.connections).toHaveLength(1);

      // Remove node-1
      useSessionStore.getState().removeNode(TEST_SESSION_ID, 'node-1');

      const afterSession = useSessionStore.getState().getSession(TEST_SESSION_ID);

      // Connection should be removed
      expect(afterSession?.pipeline?.connections).toHaveLength(0);
      expect(afterSession?.pipeline?.nodes['node-1']).toBeUndefined();
    });
  });
});
