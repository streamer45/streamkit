// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, it, expect, beforeEach } from 'vitest';

import type { Pipeline } from '@/types/types';

import { useSessionStore } from './sessionStore';

describe('sessionStore', () => {
  // Extracted constant to avoid duplication (sonarjs/no-duplicate-string)
  const TEST_SESSION_ID = 'test-session-1';

  beforeEach(() => {
    // Reset store before each test
    useSessionStore.setState({ sessions: new Map() });
  });

  describe('setPipeline', () => {
    it('should set pipeline for a session', () => {
      const sessionId = TEST_SESSION_ID;
      const pipeline: Pipeline = {
        name: null,
        description: null,
        mode: 'dynamic',
        client: null,
        nodes: {
          'node-1': {
            kind: 'gain',
            params: { gain: 2.0 },
            state: null,
          },
        },
        connections: [],
      };

      useSessionStore.getState().setPipeline(sessionId, pipeline);

      const session = useSessionStore.getState().getSession(sessionId);
      expect(session?.pipeline).toEqual(pipeline);
    });

    it('should update existing pipeline', () => {
      const sessionId = TEST_SESSION_ID;
      const pipeline1: Pipeline = {
        name: null,
        description: null,
        mode: 'dynamic',
        client: null,
        nodes: {
          'node-1': {
            kind: 'gain',
            params: { gain: 2.0 },
            state: null,
          },
        },
        connections: [],
      };
      const pipeline2: Pipeline = {
        name: null,
        description: null,
        mode: 'dynamic',
        client: null,
        nodes: {
          'node-1': {
            kind: 'gain',
            params: { gain: 3.0 },
            state: null,
          },
          'node-2': {
            kind: 'passthrough',
            params: {},
            state: null,
          },
        },
        connections: [
          {
            from_node: 'node-1',
            from_pin: 'out',
            to_node: 'node-2',
            to_pin: 'in',
          },
        ],
      };

      useSessionStore.getState().setPipeline(sessionId, pipeline1);
      useSessionStore.getState().setPipeline(sessionId, pipeline2);

      const session = useSessionStore.getState().getSession(sessionId);
      expect(session?.pipeline).toEqual(pipeline2);
    });
  });

  describe('addNode', () => {
    it('should add a node to the pipeline', () => {
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
      useSessionStore.getState().addNode(sessionId, 'node-1', {
        kind: 'gain',
        params: { gain: 2.0 },
      });

      const session = useSessionStore.getState().getSession(sessionId);
      expect(session?.pipeline?.nodes['node-1']).toBeDefined();
      expect(session?.pipeline?.nodes['node-1'].kind).toBe('gain');
    });

    it('should add node with state if provided', () => {
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
      useSessionStore.getState().addNode(sessionId, 'node-1', {
        kind: 'gain',
        params: { gain: 2.0 },
        state: 'Running',
      });

      const session = useSessionStore.getState().getSession(sessionId);
      expect(session?.pipeline?.nodes['node-1'].state).toEqual('Running');
    });
  });

  describe('removeNode', () => {
    it('should remove a node from the pipeline', () => {
      const sessionId = TEST_SESSION_ID;
      const pipeline: Pipeline = {
        name: null,
        description: null,
        mode: 'dynamic',
        client: null,
        nodes: {
          'node-1': {
            kind: 'gain',
            params: { gain: 2.0 },
            state: null,
          },
          'node-2': {
            kind: 'passthrough',
            params: {},
            state: null,
          },
        },
        connections: [],
      };

      useSessionStore.getState().setPipeline(sessionId, pipeline);
      useSessionStore.getState().removeNode(sessionId, 'node-1');

      const session = useSessionStore.getState().getSession(sessionId);
      expect(session?.pipeline?.nodes['node-1']).toBeUndefined();
      expect(session?.pipeline?.nodes['node-2']).toBeDefined();
    });
  });

  describe('addConnection', () => {
    it('should add a connection to the pipeline', () => {
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
      useSessionStore.getState().addConnection(sessionId, {
        from_node: 'node-1',
        from_pin: 'out',
        to_node: 'node-2',
        to_pin: 'in',
      });

      const session = useSessionStore.getState().getSession(sessionId);
      expect(session?.pipeline?.connections).toHaveLength(1);
      expect(session?.pipeline?.connections[0].from_node).toBe('node-1');
    });
  });

  describe('removeConnection', () => {
    it('should remove a connection from the pipeline', () => {
      const sessionId = TEST_SESSION_ID;
      const pipeline: Pipeline = {
        name: null,
        description: null,
        mode: 'dynamic',
        client: null,
        nodes: {},
        connections: [
          {
            from_node: 'node-1',
            from_pin: 'out',
            to_node: 'node-2',
            to_pin: 'in',
          },
          {
            from_node: 'node-2',
            from_pin: 'out',
            to_node: 'node-3',
            to_pin: 'in',
          },
        ],
      };

      useSessionStore.getState().setPipeline(sessionId, pipeline);
      useSessionStore.getState().removeConnection(sessionId, {
        from_node: 'node-1',
        from_pin: 'out',
        to_node: 'node-2',
        to_pin: 'in',
      });

      const session = useSessionStore.getState().getSession(sessionId);
      expect(session?.pipeline?.connections).toHaveLength(1);
      expect(session?.pipeline?.connections[0].from_node).toBe('node-2');
    });
  });

  describe('setConnected', () => {
    it('should ignore setConnected for non-existent session', () => {
      const sessionId = TEST_SESSION_ID;

      useSessionStore.getState().setConnected(sessionId, true);

      const session = useSessionStore.getState().getSession(sessionId);
      expect(session).toBeUndefined();
    });

    it('should update connection status for existing session', () => {
      const sessionId = TEST_SESSION_ID;

      useSessionStore.getState().initSession(sessionId, true);
      useSessionStore.getState().setConnected(sessionId, false);

      const session = useSessionStore.getState().getSession(sessionId);
      expect(session?.isConnected).toBe(false);
    });
  });

  describe('clearSession', () => {
    it('should remove a session', () => {
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
      useSessionStore.getState().clearSession(sessionId);

      const session = useSessionStore.getState().getSession(sessionId);
      expect(session).toBeUndefined();
    });
  });

  describe('getSession', () => {
    it('should return undefined for non-existent session', () => {
      const session = useSessionStore.getState().getSession('non-existent');
      expect(session).toBeUndefined();
    });

    it('should return session data for existing session', () => {
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
      expect(session).toBeDefined();
      expect(session?.pipeline).toEqual(pipeline);
    });
  });

  // Edge cases moved to sessionStore.edge-cases.test.ts to comply with max-lines rule
});
