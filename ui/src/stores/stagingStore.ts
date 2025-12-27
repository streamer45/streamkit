// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { debounce, throttle } from 'lodash-es';
import { create } from 'zustand';
import { persist, createJSONStorage } from 'zustand/middleware';

import type { Connection, Node, Pipeline } from '@/types/types';

import { useNodeParamsStore } from './nodeParamsStore';

export type StagingMode = 'monitor' | 'staging';

export interface ValidationError {
  type: 'error' | 'warning';
  message: string;
  nodeId?: string;
  connectionId?: string;
}

export interface StagedChange {
  type: 'add_node' | 'remove_node' | 'add_connection' | 'remove_connection' | 'update_params';
  nodeId?: string;
  connection?: Connection;
  timestamp: number;
}

export interface StagingData {
  mode: StagingMode;
  sessionId: string | null;
  // The original live pipeline when entering staging mode (for comparison)
  originalPipeline: Pipeline | null;
  // The staged pipeline (includes both live nodes and staged nodes)
  stagedPipeline: Pipeline | null;
  // Set of node IDs that are staged (not yet committed)
  stagedNodes: Set<string>;
  // Set of connections that are staged (not yet committed)
  stagedConnections: Set<string>; // Serialized as "from_node:from_pin:to_node:to_pin"
  // Parameters for staged nodes (separate from pipeline to avoid polluting live data)
  stagedParams: Record<string, Record<string, unknown>>;
  // Node positions on canvas (x, y coordinates)
  nodePositions: Record<string, { x: number; y: number }>;
  // Validation errors that block commit
  validationErrors: ValidationError[];
  // Change log for displaying summary
  changes: StagedChange[];
  // Version counter to force re-renders
  version: number;
}

interface StagingStore {
  staging: Record<string, StagingData>; // Keyed by session ID

  // Actions
  enterStagingMode: (sessionId: string, livePipeline: Pipeline) => void;
  exitStagingMode: (sessionId: string) => void;

  // Local staging operations (don't send to server)
  addStagedNode: (sessionId: string, nodeId: string, node: Node) => void;
  removeStagedNode: (sessionId: string, nodeId: string) => void;
  addStagedConnection: (sessionId: string, connection: Connection) => void;
  removeStagedConnection: (sessionId: string, connection: Connection) => void;
  updateStagedNodeParams: (
    sessionId: string,
    nodeId: string,
    params: Record<string, unknown>
  ) => void;
  updateNodePosition: (
    sessionId: string,
    nodeId: string,
    position: { x: number; y: number }
  ) => void;

  // Validation
  setValidationErrors: (sessionId: string, errors: ValidationError[]) => void;

  // Reset/discard
  discardChanges: (sessionId: string) => void;

  // Getters
  getStagingData: (sessionId: string) => StagingData | undefined;
  isInStagingMode: (sessionId: string) => boolean;
  getStagedPipeline: (sessionId: string) => Pipeline | null;
  getNodePositions: (sessionId: string) => Record<string, { x: number; y: number }>;
  getChangesSummary: (sessionId: string) => { added: number; removed: number; modified: number };
}

// Helper to serialize connection for Set storage
const serializeConnection = (conn: Connection): string =>
  `${conn.from_node}:${conn.from_pin}:${conn.to_node}:${conn.to_pin}`;

// Debounced staging store update to avoid excessive re-renders on text input
const debouncedStagingUpdates = new Map<string, ReturnType<typeof debounce>>();

const getDebouncedUpdateForNode = (sessionId: string, nodeId: string, updateFn: () => void) => {
  const key = `${sessionId}:${nodeId}`;
  if (!debouncedStagingUpdates.has(key)) {
    debouncedStagingUpdates.set(key, debounce(updateFn, 300, { leading: false, trailing: true }));
  }
  return debouncedStagingUpdates.get(key)!;
};

// Throttled localStorage wrapper to avoid excessive writes during position updates
const throttledSetItem = throttle(
  (name: string, value: string) => {
    try {
      localStorage.setItem(name, value);
    } catch {
      // ignore
    }
  },
  500,
  { leading: false, trailing: true }
);

const throttledStorage = {
  getItem: (name: string) => {
    try {
      return localStorage.getItem(name);
    } catch {
      return null;
    }
  },
  setItem: (name: string, value: string) => {
    throttledSetItem(name, value);
  },
  removeItem: (name: string) => {
    try {
      localStorage.removeItem(name);
    } catch {
      // ignore
    }
  },
};

export const useStagingStore = create<StagingStore>()(
  persist(
    (set, get) => ({
      staging: {},

      enterStagingMode: (sessionId, livePipeline) =>
        set((state) => ({
          staging: {
            ...state.staging,
            [sessionId]: {
              mode: 'staging',
              sessionId,
              // Store original pipeline for comparison when reverting changes
              originalPipeline: JSON.parse(JSON.stringify(livePipeline)) as Pipeline,
              // Deep clone the live pipeline to create the staging pipeline
              stagedPipeline: JSON.parse(JSON.stringify(livePipeline)) as Pipeline,
              stagedNodes: new Set(),
              stagedConnections: new Set(),
              stagedParams: {},
              nodePositions: {},
              validationErrors: [],
              changes: [],
              version: 0,
            },
          },
        })),

      exitStagingMode: (sessionId) =>
        set((state) => {
          // eslint-disable-next-line @typescript-eslint/no-unused-vars
          const { [sessionId]: _removed, ...rest } = state.staging;
          return { staging: rest };
        }),

      addStagedNode: (sessionId, nodeId, node) =>
        set((state) => {
          const data = state.staging[sessionId];
          if (!data || !data.stagedPipeline) return state;

          // Create completely new pipeline object with new nodes map
          const newNodes = new Map(Object.entries(data.stagedPipeline.nodes));
          newNodes.set(nodeId, node);

          const newStagedPipeline: Pipeline = {
            name: data.stagedPipeline.name,
            description: data.stagedPipeline.description,
            mode: data.stagedPipeline.mode,
            nodes: Object.fromEntries(newNodes) as Record<string, Node>,
            connections: [...data.stagedPipeline.connections],
          };

          const newStagedNodes = new Set(data.stagedNodes);
          newStagedNodes.add(nodeId);

          const newChanges: StagedChange[] = [
            ...data.changes,
            { type: 'add_node', nodeId, timestamp: Date.now() },
          ];

          // Create completely new staging data object
          const newStagingData = {
            mode: data.mode,
            sessionId: data.sessionId,
            originalPipeline: data.originalPipeline,
            stagedPipeline: newStagedPipeline,
            stagedNodes: newStagedNodes,
            stagedConnections: data.stagedConnections,
            stagedParams: data.stagedParams,
            nodePositions: data.nodePositions,
            validationErrors: data.validationErrors,
            changes: newChanges,
            version: data.version + 1,
          };

          return {
            staging: {
              ...state.staging,
              [sessionId]: newStagingData,
            },
          };
        }),

      removeStagedNode: (sessionId, nodeId) =>
        set((state) => {
          const data = state.staging[sessionId];
          if (!data || !data.stagedPipeline) return state;

          // eslint-disable-next-line @typescript-eslint/no-unused-vars
          const { [nodeId]: _removed, ...remainingNodes } = data.stagedPipeline.nodes;

          // Remove connections related to this node
          const remainingConnections = data.stagedPipeline.connections.filter(
            (c) => c.from_node !== nodeId && c.to_node !== nodeId
          );

          const newStagedPipeline: Pipeline = {
            ...data.stagedPipeline,
            nodes: remainingNodes,
            connections: remainingConnections,
          };

          const newStagedNodes = new Set(data.stagedNodes);
          const wasStaged = newStagedNodes.has(nodeId);
          newStagedNodes.delete(nodeId);

          const newStagedConnections = new Set(data.stagedConnections);
          // Remove staged connections that involve this node
          data.stagedPipeline.connections
            .filter((c) => c.from_node === nodeId || c.to_node === nodeId)
            .forEach((c) => newStagedConnections.delete(serializeConnection(c)));

          // If the node was newly added in staging mode, cancel out the add by removing it from changes
          // Otherwise, add a remove_node change for a live node being deleted
          let newChanges: StagedChange[];
          if (wasStaged) {
            // Remove the corresponding add_node change and any update_params changes
            newChanges = data.changes.filter(
              (c) => !(c.nodeId === nodeId && (c.type === 'add_node' || c.type === 'update_params'))
            );
          } else {
            // Add remove_node change for live node
            newChanges = [...data.changes, { type: 'remove_node', nodeId, timestamp: Date.now() }];
          }

          return {
            staging: {
              ...state.staging,
              [sessionId]: {
                ...data,
                stagedPipeline: newStagedPipeline,
                stagedNodes: newStagedNodes,
                stagedConnections: newStagedConnections,
                changes: newChanges,
                version: data.version + 1,
              },
            },
          };
        }),

      addStagedConnection: (sessionId, connection) =>
        set((state) => {
          const data = state.staging[sessionId];
          if (!data || !data.stagedPipeline) return state;

          const newStagedPipeline: Pipeline = {
            name: data.stagedPipeline.name,
            description: data.stagedPipeline.description,
            mode: data.stagedPipeline.mode,
            nodes: { ...data.stagedPipeline.nodes },
            connections: [...data.stagedPipeline.connections, connection],
          };

          const connectionKey = serializeConnection(connection);
          const newStagedConnections = new Set(data.stagedConnections);

          // Check if we're re-adding a connection that was previously removed in staging
          // If there's a remove_connection change for this connection, it means it was originally
          // in the live pipeline and we're just undoing the removal
          const hasRemoveChange = data.changes.some(
            (c) =>
              c.type === 'remove_connection' &&
              c.connection &&
              serializeConnection(c.connection) === connectionKey
          );

          let newChanges: StagedChange[];
          if (hasRemoveChange) {
            // Cancel out the remove by removing it from changes - back to original state
            newChanges = data.changes.filter(
              (c) =>
                !(
                  c.type === 'remove_connection' &&
                  c.connection &&
                  serializeConnection(c.connection) === connectionKey
                )
            );
            // Don't add to stagedConnections since this is a live connection
          } else {
            // This is a new connection being added in staging mode
            newStagedConnections.add(connectionKey);
            newChanges = [
              ...data.changes,
              { type: 'add_connection', connection, timestamp: Date.now() },
            ];
          }

          const newStagingData = {
            mode: data.mode,
            sessionId: data.sessionId,
            originalPipeline: data.originalPipeline,
            stagedPipeline: newStagedPipeline,
            stagedNodes: data.stagedNodes,
            stagedConnections: newStagedConnections,
            stagedParams: data.stagedParams,
            nodePositions: data.nodePositions,
            validationErrors: data.validationErrors,
            changes: newChanges,
            version: data.version + 1,
          };

          return {
            staging: {
              ...state.staging,
              [sessionId]: newStagingData,
            },
          };
        }),

      removeStagedConnection: (sessionId, connection) =>
        set((state) => {
          const data = state.staging[sessionId];
          if (!data || !data.stagedPipeline) return state;

          const connectionKey = serializeConnection(connection);
          const newConnections = data.stagedPipeline.connections.filter(
            (c) => serializeConnection(c) !== connectionKey
          );

          const newStagedPipeline: Pipeline = {
            ...data.stagedPipeline,
            connections: newConnections,
          };

          const newStagedConnections = new Set(data.stagedConnections);
          const wasStaged = newStagedConnections.has(connectionKey);
          newStagedConnections.delete(connectionKey);

          // If the connection was newly added in staging mode, cancel out the add
          // Otherwise, add a remove_connection change for a live connection being deleted
          let newChanges: StagedChange[];
          if (wasStaged) {
            // Remove the corresponding add_connection change
            newChanges = data.changes.filter(
              (c) =>
                !(
                  c.type === 'add_connection' &&
                  c.connection &&
                  serializeConnection(c.connection) === connectionKey
                )
            );
          } else {
            // Add remove_connection change for live connection
            newChanges = [
              ...data.changes,
              { type: 'remove_connection', connection, timestamp: Date.now() },
            ];
          }

          return {
            staging: {
              ...state.staging,
              [sessionId]: {
                ...data,
                stagedPipeline: newStagedPipeline,
                stagedConnections: newStagedConnections,
                changes: newChanges,
                version: data.version + 1,
              },
            },
          };
        }),

      updateStagedNodeParams: (sessionId, nodeId, params) => {
        // Immediately update nodeParams store for instant UI feedback
        const paramsStore = useNodeParamsStore.getState();
        Object.entries(params).forEach(([key, value]) => {
          paramsStore.setParam(nodeId, key, value, sessionId);
        });

        // Debounce the staging store update to avoid excessive re-renders
        const updateStaging = () => {
          set((state) => {
            const data = state.staging[sessionId];
            if (!data || !data.stagedPipeline) return state;

            // Update the staged pipeline with accumulated params
            const currentNodeParams = paramsStore.getParamsForNode(nodeId, sessionId) || {};
            const existingParams = data.stagedPipeline.nodes[nodeId].params;

            // Type guard: merge params only if existing params is an object
            const mergedParams =
              existingParams && typeof existingParams === 'object' && !Array.isArray(existingParams)
                ? { ...existingParams, ...currentNodeParams }
                : currentNodeParams;

            const updatedNode = {
              ...data.stagedPipeline.nodes[nodeId],
              params: mergedParams,
            };

            const newStagedPipeline: Pipeline = {
              ...data.stagedPipeline,
              nodes: {
                ...data.stagedPipeline.nodes,
                [nodeId]: updatedNode,
              },
            };

            // Check if parameters match the original values
            const originalNode = data.originalPipeline?.nodes[nodeId];
            const originalParams = originalNode?.params || {};
            const currentParams = updatedNode.params || {};

            // Deep comparison of parameters
            const paramsMatchOriginal =
              JSON.stringify(originalParams) === JSON.stringify(currentParams);

            // Manage the update_params change
            const hasExistingParamChange = data.changes.some(
              (c) => c.type === 'update_params' && c.nodeId === nodeId
            );

            let newChanges: StagedChange[];
            if (paramsMatchOriginal) {
              // Parameters match original - remove any update_params change
              newChanges = data.changes.filter(
                (c) => !(c.type === 'update_params' && c.nodeId === nodeId)
              );
            } else if (!hasExistingParamChange) {
              // Parameters differ and no change exists yet - add one
              newChanges = [
                ...data.changes,
                { type: 'update_params' as const, nodeId, timestamp: Date.now() },
              ];
            } else {
              // Parameters differ and change already exists - keep existing changes
              newChanges = data.changes;
            }

            return {
              staging: {
                ...state.staging,
                [sessionId]: {
                  ...data,
                  stagedPipeline: newStagedPipeline,
                  changes: newChanges,
                  version: data.version + 1,
                },
              },
            };
          });
        };

        getDebouncedUpdateForNode(sessionId, nodeId, updateStaging)();
      },

      updateNodePosition: (sessionId, nodeId, position) =>
        set((state) => {
          const data = state.staging[sessionId];
          if (!data) return state;

          return {
            staging: {
              ...state.staging,
              [sessionId]: {
                ...data,
                nodePositions: {
                  ...data.nodePositions,
                  [nodeId]: position,
                },
              },
            },
          };
        }),

      setValidationErrors: (sessionId, errors) =>
        set((state) => {
          const data = state.staging[sessionId];
          if (!data) return state;

          return {
            staging: {
              ...state.staging,
              [sessionId]: {
                ...data,
                validationErrors: errors,
              },
            },
          };
        }),

      discardChanges: (sessionId) =>
        set((state) => {
          // eslint-disable-next-line @typescript-eslint/no-unused-vars
          const { [sessionId]: _removed, ...rest } = state.staging;
          return { staging: rest };
        }),

      getStagingData: (sessionId) => {
        return get().staging[sessionId];
      },

      isInStagingMode: (sessionId) => {
        const data = get().staging[sessionId];
        return data?.mode === 'staging';
      },

      getStagedPipeline: (sessionId) => {
        const data = get().staging[sessionId];
        return data?.stagedPipeline ?? null;
      },

      getNodePositions: (sessionId) => {
        const data = get().staging[sessionId];
        return data?.nodePositions ?? {};
      },

      getChangesSummary: (sessionId) => {
        const data = get().staging[sessionId];
        if (!data) return { added: 0, removed: 0, modified: 0 };

        const added = data.changes.filter(
          (c) => c.type === 'add_node' || c.type === 'add_connection'
        ).length;
        const removed = data.changes.filter(
          (c) => c.type === 'remove_node' || c.type === 'remove_connection'
        ).length;
        const modified = data.changes.filter((c) => c.type === 'update_params').length;

        return { added, removed, modified };
      },
    }),
    {
      name: 'staging-storage',
      version: 1,
      storage: createJSONStorage(() => throttledStorage),
      partialize: (state) => ({
        staging: Object.fromEntries(
          Object.entries(state.staging).map(([sessionId, data]) => [
            sessionId,
            {
              ...data,
              stagedNodes: Array.from(data.stagedNodes),
              stagedConnections: Array.from(data.stagedConnections),
            },
          ])
        ),
      }),
      merge: (persistedState: unknown, currentState) => {
        const state = persistedState as { staging?: Record<string, unknown> };
        if (!state?.staging) return currentState;

        // Convert arrays back to Sets
        const staging: Record<string, StagingData> = {};
        Object.entries(state.staging).forEach(([sessionId, data]: [string, unknown]) => {
          const typedData = data as Partial<StagingData> & {
            stagedNodes?: unknown[];
            stagedConnections?: unknown[];
          };
          staging[sessionId] = {
            ...typedData,
            stagedNodes: new Set(
              Array.isArray(typedData.stagedNodes) ? (typedData.stagedNodes as string[]) : []
            ),
            stagedConnections: new Set(
              Array.isArray(typedData.stagedConnections)
                ? (typedData.stagedConnections as string[])
                : []
            ),
          } as StagingData;
        });

        return {
          ...currentState,
          staging,
        };
      },
    }
  )
);
