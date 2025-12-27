// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';

import type { Pipeline, Node, Connection } from '@/types/types';

import { useStagingStore } from './stagingStore';

// Mock node params store with actual param tracking
const mockNodeParams = new Map<string, Record<string, unknown>>();

vi.mock('./nodeParamsStore', () => ({
  useNodeParamsStore: {
    getState: vi.fn().mockReturnValue({
      setParam: vi.fn((nodeId: string, key: string, value: unknown) => {
        const params = mockNodeParams.get(nodeId) || {};
        params[key] = value;
        mockNodeParams.set(nodeId, params);
      }),
      getParamsForNode: vi.fn((nodeId: string) => mockNodeParams.get(nodeId) || {}),
    }),
  },
}));

// Mock localStorage
const mockLocalStorage = (() => {
  let store: Record<string, string> = {};
  return {
    getItem: (key: string) => store[key] || null,
    setItem: (key: string, value: string) => {
      store[key] = value;
    },
    removeItem: (key: string) => {
      delete store[key];
    },
    clear: () => {
      store = {};
    },
  };
})();

global.localStorage = mockLocalStorage as never;

describe('stagingStore', () => {
  const TEST_SESSION_ID = 'test-session-1';
  const TEST_LIVE_PIPELINE: Pipeline = {
    name: null,
    description: null,
    mode: 'dynamic',
    nodes: {
      'node-1': { kind: 'core::passthrough', params: { gain: 1.0 }, state: 'Initializing' },
      'node-2': { kind: 'core::gain', params: { gain: 0.5 }, state: 'Initializing' },
    },
    connections: [{ from_node: 'node-1', from_pin: 'output', to_node: 'node-2', to_pin: 'input' }],
  };

  beforeEach(() => {
    mockLocalStorage.clear();
    mockNodeParams.clear();

    // Reset store state
    useStagingStore.setState({ staging: {} });
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  describe('enterStagingMode', () => {
    it('should create staging data with cloned pipeline', () => {
      useStagingStore.getState().enterStagingMode(TEST_SESSION_ID, TEST_LIVE_PIPELINE);

      const data = useStagingStore.getState().getStagingData(TEST_SESSION_ID);

      expect(data).toBeDefined();
      expect(data?.mode).toBe('staging');
      expect(data?.originalPipeline).toEqual(TEST_LIVE_PIPELINE);
      expect(data?.stagedPipeline).toEqual(TEST_LIVE_PIPELINE);
      expect(data?.stagedNodes.size).toBe(0);
      expect(data?.stagedConnections.size).toBe(0);
    });

    it('should deep clone pipeline (not reference)', () => {
      useStagingStore.getState().enterStagingMode(TEST_SESSION_ID, TEST_LIVE_PIPELINE);

      const data = useStagingStore.getState().getStagingData(TEST_SESSION_ID);

      // Modify staged pipeline
      if (data?.stagedPipeline) {
        data.stagedPipeline.nodes['node-1'].params = { gain: 2.0 };
      }

      // Original pipeline should be unchanged
      expect(data?.originalPipeline?.nodes['node-1'].params).toEqual({ gain: 1.0 });
    });
  });

  describe('addStagedNode', () => {
    beforeEach(() => {
      useStagingStore.getState().enterStagingMode(TEST_SESSION_ID, TEST_LIVE_PIPELINE);
    });

    it('should add node to staged pipeline', () => {
      const newNode: Node = {
        kind: 'core::mixer',
        params: { num_inputs: 2 },
        state: 'Initializing',
      };

      useStagingStore.getState().addStagedNode(TEST_SESSION_ID, 'node-3', newNode);

      const data = useStagingStore.getState().getStagingData(TEST_SESSION_ID);

      expect(data?.stagedPipeline?.nodes['node-3']).toEqual(newNode);
      expect(data?.stagedNodes.has('node-3')).toBe(true);
    });

    it('should add add_node change', () => {
      const newNode: Node = { kind: 'core::mixer', params: {}, state: 'Initializing' };

      useStagingStore.getState().addStagedNode(TEST_SESSION_ID, 'node-3', newNode);

      const data = useStagingStore.getState().getStagingData(TEST_SESSION_ID);

      expect(data?.changes).toHaveLength(1);
      expect(data?.changes[0].type).toBe('add_node');
      expect(data?.changes[0].nodeId).toBe('node-3');
    });

    it('should increment version counter', () => {
      const initialData = useStagingStore.getState().getStagingData(TEST_SESSION_ID);
      const initialVersion = initialData?.version ?? 0;

      useStagingStore.getState().addStagedNode(TEST_SESSION_ID, 'node-3', {
        kind: 'core::gain',
        params: {},
        state: 'Initializing',
      });

      const newData = useStagingStore.getState().getStagingData(TEST_SESSION_ID);

      expect(newData?.version).toBe(initialVersion + 1);
    });
  });

  describe('removeStagedNode - Change Cancellation', () => {
    beforeEach(() => {
      useStagingStore.getState().enterStagingMode(TEST_SESSION_ID, TEST_LIVE_PIPELINE);
    });

    it('should cancel add_node change when removing newly added node', () => {
      // Add node
      useStagingStore.getState().addStagedNode(TEST_SESSION_ID, 'node-3', {
        kind: 'core::gain',
        params: {},
        state: 'Initializing',
      });

      let data = useStagingStore.getState().getStagingData(TEST_SESSION_ID);
      expect(data?.changes).toHaveLength(1);
      expect(data?.changes[0].type).toBe('add_node');

      // Remove the same node
      useStagingStore.getState().removeStagedNode(TEST_SESSION_ID, 'node-3');

      data = useStagingStore.getState().getStagingData(TEST_SESSION_ID);

      // Changes should be empty (add cancelled by remove)
      expect(data?.changes).toHaveLength(0);
      expect(data?.stagedNodes.has('node-3')).toBe(false);
    });

    it('should add remove_node change when removing live node', () => {
      // Remove a live node (exists in original pipeline)
      useStagingStore.getState().removeStagedNode(TEST_SESSION_ID, 'node-1');

      const data = useStagingStore.getState().getStagingData(TEST_SESSION_ID);

      expect(data?.stagedPipeline?.nodes['node-1']).toBeUndefined();
      expect(data?.changes).toHaveLength(1);
      expect(data?.changes[0].type).toBe('remove_node');
      expect(data?.changes[0].nodeId).toBe('node-1');
    });

    it('should remove connections related to removed node', () => {
      // Remove node-1 which has a connection
      useStagingStore.getState().removeStagedNode(TEST_SESSION_ID, 'node-1');

      const data = useStagingStore.getState().getStagingData(TEST_SESSION_ID);

      expect(data?.stagedPipeline?.connections).toHaveLength(0);
    });
  });

  describe('addStagedConnection - Change Cancellation', () => {
    beforeEach(() => {
      useStagingStore.getState().enterStagingMode(TEST_SESSION_ID, TEST_LIVE_PIPELINE);
    });

    it('should add connection and record change', () => {
      const newConnection: Connection = {
        from_node: 'node-2',
        from_pin: 'output',
        to_node: 'node-1',
        to_pin: 'input',
      };

      useStagingStore.getState().addStagedConnection(TEST_SESSION_ID, newConnection);

      const data = useStagingStore.getState().getStagingData(TEST_SESSION_ID);

      expect(data?.stagedPipeline?.connections).toHaveLength(2);
      expect(data?.changes).toHaveLength(1);
      expect(data?.changes[0].type).toBe('add_connection');
    });

    it('should cancel remove_connection change when re-adding removed connection', () => {
      const liveConnection = TEST_LIVE_PIPELINE.connections[0];

      // Remove live connection
      useStagingStore.getState().removeStagedConnection(TEST_SESSION_ID, liveConnection);

      let data = useStagingStore.getState().getStagingData(TEST_SESSION_ID);
      expect(data?.changes).toHaveLength(1);
      expect(data?.changes[0].type).toBe('remove_connection');

      // Re-add the same connection
      useStagingStore.getState().addStagedConnection(TEST_SESSION_ID, liveConnection);

      data = useStagingStore.getState().getStagingData(TEST_SESSION_ID);

      // Changes should be empty (remove cancelled by add)
      expect(data?.changes).toHaveLength(0);
    });
  });

  describe('removeStagedConnection - Change Cancellation', () => {
    beforeEach(() => {
      useStagingStore.getState().enterStagingMode(TEST_SESSION_ID, TEST_LIVE_PIPELINE);
    });

    it('should cancel add_connection change when removing newly added connection', () => {
      const newConnection: Connection = {
        from_node: 'node-2',
        from_pin: 'output',
        to_node: 'node-1',
        to_pin: 'input',
      };

      // Add connection
      useStagingStore.getState().addStagedConnection(TEST_SESSION_ID, newConnection);

      let data = useStagingStore.getState().getStagingData(TEST_SESSION_ID);
      expect(data?.changes).toHaveLength(1);

      // Remove the same connection
      useStagingStore.getState().removeStagedConnection(TEST_SESSION_ID, newConnection);

      data = useStagingStore.getState().getStagingData(TEST_SESSION_ID);

      // Changes should be empty
      expect(data?.changes).toHaveLength(0);
    });

    it('should add remove_connection change when removing live connection', () => {
      const liveConnection = TEST_LIVE_PIPELINE.connections[0];

      useStagingStore.getState().removeStagedConnection(TEST_SESSION_ID, liveConnection);

      const data = useStagingStore.getState().getStagingData(TEST_SESSION_ID);

      expect(data?.stagedPipeline?.connections).toHaveLength(0);
      expect(data?.changes).toHaveLength(1);
      expect(data?.changes[0].type).toBe('remove_connection');
    });
  });

  describe('updateStagedNodeParams - Debouncing', () => {
    beforeEach(() => {
      useStagingStore.getState().enterStagingMode(TEST_SESSION_ID, TEST_LIVE_PIPELINE);
    });

    it('should debounce param updates (300ms)', async () => {
      // Rapidly update params 3 times
      useStagingStore.getState().updateStagedNodeParams(TEST_SESSION_ID, 'node-1', { gain: 1.5 });
      useStagingStore.getState().updateStagedNodeParams(TEST_SESSION_ID, 'node-1', { gain: 2.0 });
      useStagingStore.getState().updateStagedNodeParams(TEST_SESSION_ID, 'node-1', { gain: 2.5 });

      // Changes should not be updated yet
      let data = useStagingStore.getState().getStagingData(TEST_SESSION_ID);
      expect(data?.changes).toHaveLength(0);

      // Wait for debounce delay (300ms + buffer)
      await new Promise((resolve) => setTimeout(resolve, 350));

      data = useStagingStore.getState().getStagingData(TEST_SESSION_ID);

      // Now changes should be updated (only once despite 3 updates)
      expect(data?.changes).toHaveLength(1);
      expect(data?.changes[0].type).toBe('update_params');
      expect(data?.changes[0].nodeId).toBe('node-1');
    });

    it('should remove update_params change when params match original', async () => {
      // Update params to different value
      useStagingStore.getState().updateStagedNodeParams(TEST_SESSION_ID, 'node-1', { gain: 2.0 });
      await new Promise((resolve) => setTimeout(resolve, 350));

      let data = useStagingStore.getState().getStagingData(TEST_SESSION_ID);
      expect(data?.changes).toHaveLength(1);

      // Revert to original value
      useStagingStore.getState().updateStagedNodeParams(TEST_SESSION_ID, 'node-1', { gain: 1.0 });
      await new Promise((resolve) => setTimeout(resolve, 350));

      data = useStagingStore.getState().getStagingData(TEST_SESSION_ID);

      // Change should be removed (params match original)
      expect(data?.changes).toHaveLength(0);
    });
  });

  describe('Persistence - Set Serialization', () => {
    it('should serialize Sets to Arrays for JSON storage', () => {
      useStagingStore.getState().enterStagingMode(TEST_SESSION_ID, TEST_LIVE_PIPELINE);
      useStagingStore.getState().addStagedNode(TEST_SESSION_ID, 'node-3', {
        kind: 'core::gain',
        params: {},
        state: 'Initializing',
      });
      useStagingStore.getState().addStagedConnection(TEST_SESSION_ID, {
        from_node: 'node-3',
        from_pin: 'output',
        to_node: 'node-1',
        to_pin: 'input',
      });

      // Trigger persistence (in real app this happens via middleware)
      const state = useStagingStore.getState();

      // Access the persist config to test partialize
      const persistConfig = useStagingStore.persist?.getOptions?.();
      if (persistConfig?.partialize) {
        const serialized = persistConfig.partialize(state) as {
          staging: Record<string, { stagedNodes: string[]; stagedConnections: unknown[] }>;
        };

        const sessionData = serialized.staging[TEST_SESSION_ID];
        // Sets should be serialized as Arrays
        expect(Array.isArray(sessionData.stagedNodes)).toBe(true);
        expect(Array.isArray(sessionData.stagedConnections)).toBe(true);
        expect(sessionData.stagedNodes).toContain('node-3');
      }
    });
  });

  describe('getChangesSummary', () => {
    beforeEach(() => {
      useStagingStore.getState().enterStagingMode(TEST_SESSION_ID, TEST_LIVE_PIPELINE);
    });

    it('should count added changes', async () => {
      useStagingStore.getState().addStagedNode(TEST_SESSION_ID, 'node-3', {
        kind: 'core::gain',
        params: {},
        state: 'Initializing',
      });
      useStagingStore.getState().addStagedConnection(TEST_SESSION_ID, {
        from_node: 'node-3',
        from_pin: 'output',
        to_node: 'node-1',
        to_pin: 'input',
      });

      const summary = useStagingStore.getState().getChangesSummary(TEST_SESSION_ID);

      expect(summary.added).toBe(2);
      expect(summary.removed).toBe(0);
      expect(summary.modified).toBe(0);
    });

    it('should count removed changes', () => {
      useStagingStore.getState().removeStagedNode(TEST_SESSION_ID, 'node-1');

      const summary = useStagingStore.getState().getChangesSummary(TEST_SESSION_ID);

      expect(summary.added).toBe(0);
      expect(summary.removed).toBe(1); // Node removal (connection removed automatically)
      expect(summary.modified).toBe(0);
    });

    it('should count modified changes', async () => {
      useStagingStore.getState().updateStagedNodeParams(TEST_SESSION_ID, 'node-1', { gain: 2.0 });
      await new Promise((resolve) => setTimeout(resolve, 350));

      const summary = useStagingStore.getState().getChangesSummary(TEST_SESSION_ID);

      expect(summary.added).toBe(0);
      expect(summary.removed).toBe(0);
      expect(summary.modified).toBe(1);
    });
  });

  describe('exitStagingMode', () => {
    it('should remove staging data for session', () => {
      useStagingStore.getState().enterStagingMode(TEST_SESSION_ID, TEST_LIVE_PIPELINE);

      expect(useStagingStore.getState().isInStagingMode(TEST_SESSION_ID)).toBe(true);

      useStagingStore.getState().exitStagingMode(TEST_SESSION_ID);

      expect(useStagingStore.getState().isInStagingMode(TEST_SESSION_ID)).toBe(false);
      expect(useStagingStore.getState().getStagingData(TEST_SESSION_ID)).toBeUndefined();
    });
  });

  describe('discardChanges', () => {
    it('should discard all changes and exit staging mode', () => {
      useStagingStore.getState().enterStagingMode(TEST_SESSION_ID, TEST_LIVE_PIPELINE);
      useStagingStore.getState().addStagedNode(TEST_SESSION_ID, 'node-3', {
        kind: 'core::gain',
        params: {},
        state: 'Initializing',
      });

      useStagingStore.getState().discardChanges(TEST_SESSION_ID);

      expect(useStagingStore.getState().getStagingData(TEST_SESSION_ID)).toBeUndefined();
    });
  });

  describe('setValidationErrors', () => {
    beforeEach(() => {
      useStagingStore.getState().enterStagingMode(TEST_SESSION_ID, TEST_LIVE_PIPELINE);
    });

    it('should set validation errors', () => {
      const errors = [{ type: 'error' as const, message: 'Cycle detected', nodeId: 'node-1' }];

      useStagingStore.getState().setValidationErrors(TEST_SESSION_ID, errors);

      const data = useStagingStore.getState().getStagingData(TEST_SESSION_ID);

      expect(data?.validationErrors).toEqual(errors);
    });
  });

  describe('updateNodePosition', () => {
    beforeEach(() => {
      useStagingStore.getState().enterStagingMode(TEST_SESSION_ID, TEST_LIVE_PIPELINE);
    });

    it('should update node position', () => {
      const position = { x: 100, y: 200 };

      useStagingStore.getState().updateNodePosition(TEST_SESSION_ID, 'node-1', position);

      const positions = useStagingStore.getState().getNodePositions(TEST_SESSION_ID);

      expect(positions['node-1']).toEqual(position);
    });
  });

  describe('isInStagingMode', () => {
    it('should return false when session has no staging data', () => {
      expect(useStagingStore.getState().isInStagingMode(TEST_SESSION_ID)).toBe(false);
    });

    it('should return true when in staging mode', () => {
      useStagingStore.getState().enterStagingMode(TEST_SESSION_ID, TEST_LIVE_PIPELINE);

      expect(useStagingStore.getState().isInStagingMode(TEST_SESSION_ID)).toBe(true);
    });
  });

  describe('getStagedPipeline', () => {
    it('should return null when no staging data', () => {
      expect(useStagingStore.getState().getStagedPipeline(TEST_SESSION_ID)).toBeNull();
    });

    it('should return staged pipeline', () => {
      useStagingStore.getState().enterStagingMode(TEST_SESSION_ID, TEST_LIVE_PIPELINE);

      const pipeline = useStagingStore.getState().getStagedPipeline(TEST_SESSION_ID);

      expect(pipeline).toEqual(TEST_LIVE_PIPELINE);
    });
  });
});
