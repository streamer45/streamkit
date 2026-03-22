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

      // Initialize sessions first
      useSessionStore.getState().initSession(session1, false);
      useSessionStore.getState().initSession(session2, false);

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
        client: null,
        nodes: { 'node-1': { kind: 'core::passthrough', params: {}, state: 'Initializing' } },
        connections: [],
      };
      const pipeline2: Pipeline = {
        name: null,
        description: null,
        mode: 'dynamic',
        client: null,
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

      useSessionStore.getState().initSession(TEST_SESSION_ID, false);
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
        client: null,
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
      // Create session without pipeline via initSession
      useSessionStore.getState().initSession('empty-session', false);

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

  describe('setPipeline view_data extraction', () => {
    it('should extract view_data into nodeViewData on initial load', () => {
      const sessionId = TEST_SESSION_ID;
      const pipeline: Pipeline = {
        name: null,
        description: null,
        mode: 'dynamic',
        client: null,
        nodes: {
          compositor: {
            kind: 'video::compositor',
            params: { width: 1280, height: 720 },
            state: 'Running',
          },
        },
        connections: [],
        view_data: {
          compositor: { layers: { in_0: { x: 0, y: 60, width: 1280, height: 600 } } },
        },
      };

      useSessionStore.getState().setPipeline(sessionId, pipeline);

      const session = useSessionStore.getState().getSession(sessionId);
      expect(session?.nodeViewData).toBeDefined();
      expect(session?.nodeViewData.compositor).toEqual({
        layers: { in_0: { x: 0, y: 60, width: 1280, height: 600 } },
      });
    });

    it('should merge view_data with existing nodeViewData', () => {
      const sessionId = TEST_SESSION_ID;

      // First pipeline sets initial view data
      useSessionStore.getState().setPipeline(sessionId, {
        name: null,
        description: null,
        mode: 'dynamic',
        client: null,
        nodes: {},
        connections: [],
        view_data: { nodeA: { key: 'original' } },
      });

      // Second pipeline adds more view data
      useSessionStore.getState().setPipeline(sessionId, {
        name: null,
        description: null,
        mode: 'dynamic',
        client: null,
        nodes: {},
        connections: [],
        view_data: { nodeB: { key: 'new' } },
      });

      const session = useSessionStore.getState().getSession(sessionId);
      expect(session?.nodeViewData.nodeA).toEqual({ key: 'original' });
      expect(session?.nodeViewData.nodeB).toEqual({ key: 'new' });
    });

    it('should handle null view_data gracefully', () => {
      const sessionId = TEST_SESSION_ID;
      const pipeline: Pipeline = {
        name: null,
        description: null,
        mode: 'dynamic',
        client: null,
        nodes: {},
        connections: [],
      };

      useSessionStore.getState().setPipeline(sessionId, pipeline);

      const session = useSessionStore.getState().getSession(sessionId);
      expect(session?.nodeViewData).toEqual({});
    });

    it('should extract view_data in batchSetPipelines', () => {
      const pipelines = [
        {
          sessionId: 'session-a',
          pipeline: {
            name: null,
            description: null,
            mode: 'dynamic' as const,
            client: null,
            nodes: {},
            connections: [],
            view_data: { comp: { layers: { in_0: { x: 10 } } } },
          },
        },
      ];

      useSessionStore.getState().batchSetPipelines(pipelines);

      const session = useSessionStore.getState().getSession('session-a');
      expect(session?.nodeViewData.comp).toEqual({ layers: { in_0: { x: 10 } } });
    });
  });

  describe('Pipeline Updates with Missing Nodes', () => {
    beforeEach(() => {
      const pipeline: Pipeline = {
        name: null,
        description: null,
        mode: 'dynamic',
        client: null,
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
