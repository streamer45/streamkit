// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { create } from 'zustand';

import type { Connection, Node, Pipeline, NodeState } from '@/types/types';

interface SessionData {
  pipeline: Pipeline | null;
  nodeViewData: Record<string, unknown>;
  isConnected: boolean;
}

interface SessionStore {
  sessions: Map<string, SessionData>;

  // Actions
  updateNodeViewData: (sessionId: string, nodeId: string, data: unknown) => void;
  updateRuntimeSchema: (sessionId: string, nodeId: string, schema: unknown) => void;
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
  initSession: (sessionId: string, connected: boolean) => void;
  clearSession: (sessionId: string) => void;
  getSession: (sessionId: string) => SessionData | undefined;
}

export const useSessionStore = create<SessionStore>((set, get) => ({
  sessions: new Map(),

  updateNodeViewData: (sessionId, nodeId, data) =>
    set((prev) => {
      const session = prev.sessions.get(sessionId);
      if (!session) return prev; // Ignore updates for unknown/destroyed sessions

      const newSessions = new Map(prev.sessions);
      newSessions.set(sessionId, {
        ...session,
        nodeViewData: { ...session.nodeViewData, [nodeId]: data },
      });
      return { sessions: newSessions };
    }),

  updateRuntimeSchema: (sessionId, nodeId, schema) =>
    set((prev) => {
      const session = prev.sessions.get(sessionId);
      if (!session || !session.pipeline) return prev;

      const existing = session.pipeline.runtime_schemas ?? {};
      const updatedPipeline: Pipeline = {
        ...session.pipeline,
        runtime_schemas: { ...existing, [nodeId]: schema },
      };

      const newSessions = new Map(prev.sessions);
      newSessions.set(sessionId, { ...session, pipeline: updatedPipeline });
      return { sessions: newSessions };
    }),

  setPipeline: (sessionId, pipeline) =>
    set((prev) => {
      const session = prev.sessions.get(sessionId);
      const newSessions = new Map(prev.sessions);

      // Extract view data snapshot (e.g. compositor resolved layout) so
      // useServerLayoutSync finds it immediately on mount.
      const incomingViewData =
        pipeline.view_data && typeof pipeline.view_data === 'object'
          ? (pipeline.view_data as Record<string, unknown>)
          : {};

      newSessions.set(sessionId, {
        pipeline,
        nodeViewData: { ...(session?.nodeViewData ?? {}), ...incomingViewData },
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

      let mergedParams: unknown;
      if (
        existingParams &&
        typeof existingParams === 'object' &&
        !Array.isArray(existingParams) &&
        existingParams !== null
      ) {
        mergedParams = { ...(existingParams as Record<string, unknown>), ...params };
      } else {
        mergedParams = params;
      }

      const newPipeline: Pipeline = {
        ...session.pipeline,
        nodes: {
          ...session.pipeline.nodes,
          [nodeId]: {
            ...existingNode,
            params: mergedParams,
          },
        },
      };

      newSessions.set(sessionId, { ...session, pipeline: newPipeline });
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
      if (!session) return prev; // Don't re-create destroyed/unknown sessions

      const newSessions = new Map(prev.sessions);
      newSessions.set(sessionId, {
        ...session,
        isConnected: connected,
      });
      return { sessions: newSessions };
    }),

  initSession: (sessionId, connected) =>
    set((prev) => {
      const session = prev.sessions.get(sessionId);
      if (session) {
        // Session already exists, just update connection status
        const newSessions = new Map(prev.sessions);
        newSessions.set(sessionId, { ...session, isConnected: connected });
        return { sessions: newSessions };
      }
      const newSessions = new Map(prev.sessions);
      newSessions.set(sessionId, {
        pipeline: null,
        nodeViewData: {},
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
