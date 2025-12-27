// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useQuery } from '@tanstack/react-query';
import { useEffect, useCallback } from 'react';
import { v4 as uuidv4 } from 'uuid';
import { useShallow } from 'zustand/shallow';

import { getApiUrl } from '@/services/base';
import { getWebSocketService } from '@/services/websocket';
import { useNodeParamsStore } from '@/stores/nodeParamsStore';
import { useSessionStore } from '@/stores/sessionStore';
import type { Pipeline, NodeState, Request, MessageType, BatchOperation } from '@/types/types';

const EMPTY_NODE_STATES: Record<string, NodeState> = Object.freeze({});

async function fetchPipeline(sessionId: string): Promise<Pipeline> {
  const apiUrl = getApiUrl();
  const response = await fetch(`${apiUrl}/api/v1/sessions/${sessionId}/pipeline`);
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

  // Update Zustand store when pipeline data is fetched
  useEffect(() => {
    if (pipelineQuery.data && sessionId) {
      useSessionStore.getState().setPipeline(sessionId, pipelineQuery.data);
    }
  }, [pipelineQuery.data, sessionId]);

  // Get real-time state from Zustand with granular selectors to minimize re-renders
  // Use shallow comparison for objects to prevent re-renders when object references change but content is same
  const pipeline = useSessionStore((state) =>
    sessionId ? state.getSession(sessionId)?.pipeline : undefined
  );
  const nodeStates: Record<string, NodeState> = useSessionStore(
    useShallow((state) =>
      sessionId ? (state.getSession(sessionId)?.nodeStates ?? EMPTY_NODE_STATES) : EMPTY_NODE_STATES
    )
  );
  const isConnectedFromStore = useSessionStore((state) =>
    sessionId ? (state.getSession(sessionId)?.isConnected ?? false) : false
  );

  const tuneNode = useCallback(
    (nodeId: string, param: string, value: unknown) => {
      if (!sessionId) return;

      useNodeParamsStore.getState().setParam(nodeId, param, value, sessionId);

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
    nodeStates,
    isConnected: isConnectedFromStore,
    isLoading: pipelineQuery.isLoading,
    error: pipelineQuery.error,
    tuneNode,
    addNode,
    removeNode,
    connectPins,
    disconnectPins,
    applyBatch,
  };
}
