// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { create } from 'zustand';

import type { Connection, Node, Pipeline, NodeState, NodeStats } from '@/types/types';

interface SessionData {
  pipeline: Pipeline | null;
  nodeStates: Record<string, NodeState>;
  nodeStats: Record<string, NodeStats>;
  isConnected: boolean;
}

interface SessionStore {
  sessions: Map<string, SessionData>;

  // Actions
  updateNodeState: (sessionId: string, nodeId: string, state: NodeState) => void;
  updateNodeStats: (sessionId: string, nodeId: string, stats: NodeStats) => void;
  setPipeline: (sessionId: string, pipeline: Pipeline) => void;
  updateNodeParams: (sessionId: string, nodeId: string, params: Record<string, unknown>) => void;
  addNode: (
    sessionId: string,
    nodeId: string,
    nodeData: Omit<Node, 'state'> & { state?: NodeState | null }
  ) => void;
  removeNode: (sessionId: string, nodeId: string) => void;
  addConnection: (sessionId: string, connection: Connection) => void;
  removeConnection: (sessionId: string, connection: Connection) => void;
  setConnected: (sessionId: string, connected: boolean) => void;
  clearSession: (sessionId: string) => void;
  getSession: (sessionId: string) => SessionData | undefined;
}

export const useSessionStore = create<SessionStore>((set, get) => ({
  sessions: new Map(),

  updateNodeState: (sessionId, nodeId, state) =>
    set((prev) => {
      const session = prev.sessions.get(sessionId);
      if (!session) {
        // Initialize session if it doesn't exist
        const newSessions = new Map(prev.sessions);
        newSessions.set(sessionId, {
          pipeline: null,
          nodeStates: { [nodeId]: state },
          nodeStats: {},
          isConnected: false,
        });
        return { sessions: newSessions };
      }

      const newSessions = new Map(prev.sessions);
      newSessions.set(sessionId, {
        ...session,
        nodeStates: { ...session.nodeStates, [nodeId]: state },
      });
      return { sessions: newSessions };
    }),

  updateNodeStats: (sessionId, nodeId, stats) =>
    set((prev) => {
      const session = prev.sessions.get(sessionId);
      if (!session) {
        // Initialize session if it doesn't exist
        const newSessions = new Map(prev.sessions);
        newSessions.set(sessionId, {
          pipeline: null,
          nodeStates: {},
          nodeStats: { [nodeId]: stats },
          isConnected: false,
        });
        return { sessions: newSessions };
      }

      const newSessions = new Map(prev.sessions);
      newSessions.set(sessionId, {
        ...session,
        nodeStats: { ...session.nodeStats, [nodeId]: stats },
      });
      return { sessions: newSessions };
    }),

  setPipeline: (sessionId, pipeline) =>
    set((prev) => {
      const session = prev.sessions.get(sessionId);
      const newSessions = new Map(prev.sessions);

      // Extract initial node states from pipeline
      const nodeStates: Record<string, NodeState> = {};
      if (pipeline.nodes) {
        Object.entries(pipeline.nodes).forEach(([nodeId, node]) => {
          if (node.state) {
            nodeStates[nodeId] = node.state;
          }
        });
      }

      newSessions.set(sessionId, {
        pipeline,
        nodeStates: session ? { ...session.nodeStates, ...nodeStates } : nodeStates,
        nodeStats: session?.nodeStats ?? {},
        isConnected: session?.isConnected ?? false,
      });
      return { sessions: newSessions };
    }),

  updateNodeParams: (sessionId, nodeId, params) =>
    set((prev) => {
      const session = prev.sessions.get(sessionId);
      if (!session || !session.pipeline) return prev;

      const newSessions = new Map(prev.sessions);
      const existingNode = session.pipeline.nodes[nodeId];
      const existingParams = existingNode?.params;

      // Type guard: merge params only if both are objects
      const mergedParams =
        existingParams &&
        typeof existingParams === 'object' &&
        !Array.isArray(existingParams) &&
        typeof params === 'object' &&
        !Array.isArray(params)
          ? { ...existingParams, ...params }
          : params;

      const updatedPipeline: Pipeline = {
        ...session.pipeline,
        nodes: {
          ...session.pipeline.nodes,
          [nodeId]: {
            ...existingNode,
            params: mergedParams,
          },
        },
      };

      newSessions.set(sessionId, {
        ...session,
        pipeline: updatedPipeline,
      });
      return { sessions: newSessions };
    }),

  addNode: (sessionId, nodeId, nodeData) =>
    set((prev) => {
      const session = prev.sessions.get(sessionId);
      if (!session || !session.pipeline) return prev;

      const newPipeline: Pipeline = {
        ...session.pipeline,
        nodes: {
          ...session.pipeline.nodes,
          [nodeId]: {
            kind: nodeData.kind,
            params: nodeData.params,
            state: nodeData.state ?? null,
          },
        },
      };

      const newSessions = new Map(prev.sessions);
      newSessions.set(sessionId, { ...session, pipeline: newPipeline });
      return { sessions: newSessions };
    }),

  removeNode: (sessionId, nodeId) =>
    set((prev) => {
      const session = prev.sessions.get(sessionId);
      if (!session || !session.pipeline) return prev;

      const remainingNodes = Object.fromEntries(
        Object.entries(session.pipeline.nodes).filter(([id]) => id !== nodeId)
      ) as typeof session.pipeline.nodes;
      const remainingConnections = session.pipeline.connections.filter(
        (c) => c.from_node !== nodeId && c.to_node !== nodeId
      );

      const newPipeline: Pipeline = {
        ...session.pipeline,
        nodes: remainingNodes,
        connections: remainingConnections,
      };

      const newSessions = new Map(prev.sessions);
      newSessions.set(sessionId, { ...session, pipeline: newPipeline });
      return { sessions: newSessions };
    }),

  addConnection: (sessionId, connection) =>
    set((prev) => {
      const session = prev.sessions.get(sessionId);
      if (!session || !session.pipeline) return prev;

      const newPipeline: Pipeline = {
        ...session.pipeline,
        connections: [...session.pipeline.connections, connection],
      };

      const newSessions = new Map(prev.sessions);
      newSessions.set(sessionId, { ...session, pipeline: newPipeline });
      return { sessions: newSessions };
    }),

  removeConnection: (sessionId, connection) =>
    set((prev) => {
      const session = prev.sessions.get(sessionId);
      if (!session || !session.pipeline) return prev;

      const newConnections = session.pipeline.connections.filter(
        (c) =>
          !(
            c.from_node === connection.from_node &&
            c.from_pin === connection.from_pin &&
            c.to_node === connection.to_node &&
            c.to_pin === connection.to_pin
          )
      );

      const newPipeline: Pipeline = {
        ...session.pipeline,
        connections: newConnections,
      };

      const newSessions = new Map(prev.sessions);
      newSessions.set(sessionId, { ...session, pipeline: newPipeline });
      return { sessions: newSessions };
    }),

  setConnected: (sessionId, connected) =>
    set((prev) => {
      const session = prev.sessions.get(sessionId);
      if (!session) {
        // Initialize session if it doesn't exist
        const newSessions = new Map(prev.sessions);
        newSessions.set(sessionId, {
          pipeline: null,
          nodeStates: {},
          nodeStats: {},
          isConnected: connected,
        });
        return { sessions: newSessions };
      }

      const newSessions = new Map(prev.sessions);
      newSessions.set(sessionId, {
        ...session,
        isConnected: connected,
      });
      return { sessions: newSessions };
    }),

  clearSession: (sessionId) =>
    set((prev) => {
      const newSessions = new Map(prev.sessions);
      newSessions.delete(sessionId);
      return { sessions: newSessions };
    }),

  getSession: (sessionId) => {
    return get().sessions.get(sessionId);
  },
}));

// Granular selector helpers to prevent unnecessary re-renders
// Use these to subscribe to only the specific parts of session data you need

export const selectSessionPipeline = (sessionId: string | null) => (state: SessionStore) =>
  sessionId ? (state.sessions.get(sessionId)?.pipeline ?? null) : null;

export const selectNodeStates = (sessionId: string | null) => (state: SessionStore) =>
  sessionId ? (state.sessions.get(sessionId)?.nodeStates ?? {}) : {};

export const selectNodeState =
  (sessionId: string | null, nodeId: string) => (state: SessionStore) =>
    sessionId ? state.sessions.get(sessionId)?.nodeStates[nodeId] : undefined;

export const selectNodeStats = (sessionId: string | null) => (state: SessionStore) =>
  sessionId ? (state.sessions.get(sessionId)?.nodeStats ?? {}) : {};

export const selectNodeStat =
  (sessionId: string | null, nodeId: string) => (state: SessionStore) =>
    sessionId ? state.sessions.get(sessionId)?.nodeStats[nodeId] : undefined;

export const selectSessionIsConnected = (sessionId: string | null) => (state: SessionStore) =>
  sessionId ? (state.sessions.get(sessionId)?.isConnected ?? false) : false;
