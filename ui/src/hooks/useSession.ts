// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useQuery } from '@tanstack/react-query';
import { useAtomValue } from 'jotai/react';
import { useEffect, useCallback } from 'react';
import { v4 as uuidv4 } from 'uuid';

import { fetchApi } from '@/services/base';
import { getWebSocketService } from '@/services/websocket';
import {
  sessionConnectedAtom,
  seedPipelineAtoms,
  writeNodeParam,
  writeNodeParams,
} from '@/stores/sessionAtoms';
import { useSessionStore } from '@/stores/sessionStore';
import type { Pipeline, Request, MessageType, BatchOperation } from '@/types/types';

async function fetchPipeline(sessionId: string): Promise<Pipeline> {
  const response = await fetchApi(`/api/v1/sessions/${sessionId}/pipeline`);
  if (!response.ok) {
    throw new Error(`Failed to fetch pipeline: ${response.statusText}`);
  }
  return response.json();
}

export function useSession(sessionId: string | null) {
  const wsService = getWebSocketService();

  // Subscribe to session updates via WebSocket
  useEffect(() => {
    if (!sessionId) return;

    wsService.subscribeToSession(sessionId);

    return () => {
      wsService.unsubscribeFromSession(sessionId);
    };
  }, [sessionId, wsService]);

  // Fetch initial pipeline data
  const pipelineQuery = useQuery({
    queryKey: ['pipeline', sessionId],
    queryFn: () => fetchPipeline(sessionId!),
    enabled: !!sessionId,
    staleTime: Infinity, // WebSocket keeps it fresh
  });

  // Update Zustand store and seed Jotai atoms when pipeline data is fetched
  useEffect(() => {
    if (pipelineQuery.data && sessionId) {
      useSessionStore.getState().setPipeline(sessionId, pipelineQuery.data);
      seedPipelineAtoms(sessionId, pipelineQuery.data);
    }
  }, [pipelineQuery.data, sessionId]);

  // Get real-time state from Zustand with granular selectors to minimize re-renders.
  // nodeStates is intentionally NOT subscribed here — it changes on every WS
  // node-state event and would force the (very large) MonitorViewContent to
  // re-render each time.  MonitorViewContent patches ReactFlow nodes directly
  // via a Zustand store subscription instead.
  const pipeline = useSessionStore((state) =>
    sessionId ? state.getSession(sessionId)?.pipeline : undefined
  );
  // Read connection status from Jotai atom (fine-grained, per-session).
  const isConnectedFromStore = useAtomValue(sessionConnectedAtom(sessionId ?? ''));

  const tuneNode = useCallback(
    (nodeId: string, param: string, value: unknown) => {
      if (!sessionId) return;

      writeNodeParam(nodeId, param, value, sessionId);

      const request: Request = {
        type: 'request' as MessageType,
        correlation_id: uuidv4(),
        payload: {
          action: 'tunenodeasync' as const,
          session_id: sessionId,
          node_id: nodeId,
          message: {
            UpdateParams: { [param]: value },
          },
        },
      };

      // Fire-and-forget WebSocket message; no optimistic global state mutation
      wsService.sendFireAndForget(request);
    },
    [sessionId, wsService]
  );

  // Send a full config object as a single UpdateParams message.
  // Unlike tuneNode (which sends one key-value pair), this sends the entire
  // config so nodes like the compositor don't lose fields due to #[serde(default)].
  const tuneNodeConfig = useCallback(
    (nodeId: string, config: Record<string, unknown>) => {
      if (!sessionId) return;

      writeNodeParams(nodeId, config, sessionId);

      const request: Request = {
        type: 'request' as MessageType,
        correlation_id: uuidv4(),
        payload: {
          action: 'tunenodeasync' as const,
          session_id: sessionId,
          node_id: nodeId,
          message: {
            UpdateParams: config,
          },
        },
      };

      wsService.sendFireAndForget(request);
    },
    [sessionId, wsService]
  );

  const addNode = useCallback(
    (nodeId: string, kind: string, params: Record<string, unknown> = {}) => {
      if (!sessionId) return;

      const request: Request = {
        type: 'request' as MessageType,
        correlation_id: uuidv4(),
        payload: {
          action: 'addnode' as const,
          session_id: sessionId,
          node_id: nodeId,
          kind,
          params,
        },
      };

      wsService.sendFireAndForget(request);
    },
    [sessionId, wsService]
  );

  const removeNode = useCallback(
    (nodeId: string) => {
      if (!sessionId) return;

      const request: Request = {
        type: 'request' as MessageType,
        correlation_id: uuidv4(),
        payload: {
          action: 'removenode' as const,
          session_id: sessionId,
          node_id: nodeId,
        },
      };

      wsService.sendFireAndForget(request);
    },
    [sessionId, wsService]
  );

  const connectPins = useCallback(
    (from_node: string, from_pin: string, to_node: string, to_pin: string) => {
      if (!sessionId) return;

      const request: Request = {
        type: 'request' as MessageType,
        correlation_id: uuidv4(),
        payload: {
          action: 'connect' as const,
          session_id: sessionId,
          from_node,
          from_pin,
          to_node,
          to_pin,
          mode: 'reliable',
        },
      };

      wsService.sendFireAndForget(request);
    },
    [sessionId, wsService]
  );

  const disconnectPins = useCallback(
    (from_node: string, from_pin: string, to_node: string, to_pin: string) => {
      if (!sessionId) return;

      const request: Request = {
        type: 'request' as MessageType,
        correlation_id: uuidv4(),
        payload: {
          action: 'disconnect' as const,
          session_id: sessionId,
          from_node,
          from_pin,
          to_node,
          to_pin,
        },
      };

      wsService.sendFireAndForget(request);
    },
    [sessionId, wsService]
  );

  const applyBatch = useCallback(
    async (operations: BatchOperation[]) => {
      if (!sessionId) return;

      const request: Request = {
        type: 'request' as MessageType,
        correlation_id: uuidv4(),
        payload: {
          action: 'applybatch' as const,
          session_id: sessionId,
          operations,
        },
      };

      return wsService.send(request);
    },
    [sessionId, wsService]
  );

  return {
    pipeline: pipeline ?? pipelineQuery.data,
    isConnected: isConnectedFromStore,
    isLoading: pipelineQuery.isLoading,
    error: pipelineQuery.error,
    tuneNode,
    tuneNodeConfig,
    addNode,
    removeNode,
    connectPins,
    disconnectPins,
    applyBatch,
  };
}
