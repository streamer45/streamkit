// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useAtomValue } from 'jotai/react';
import { useCallback, useEffect } from 'react';
import { v4 as uuidv4 } from 'uuid';

import { getWebSocketService } from '@/services/websocket';
import { sessionConnectedAtom, writeNodeParam, writeNodeParams } from '@/stores/sessionAtoms';
import { useSessionStore } from '@/stores/sessionStore';
import type { Request, MessageType, BatchOperation } from '@/types/types';

export function useSession(sessionId: string | null) {
  const wsService = getWebSocketService();

  // Subscribe to session updates via WebSocket.
  // subscribeToSession also fetches the initial pipeline state over WS
  // (getpipeline), populating the session store and Jotai atoms.  This
  // eliminates the HTTP/WS race that caused stale REST data to overwrite
  // live state.
  useEffect(() => {
    if (!sessionId) return;

    wsService.subscribeToSession(sessionId);

    return () => {
      wsService.unsubscribeFromSession(sessionId);
    };
  }, [sessionId, wsService]);

  // Get real-time state from Zustand with granular selectors to minimize re-renders.
  // nodeStates is intentionally NOT subscribed here — it changes on every WS
  // node-state event and would force the (very large) MonitorViewContent to
  // re-render each time.  MonitorViewContent patches ReactFlow nodes directly
  // via a Zustand store subscription instead.
  const pipeline = useSessionStore(
    useCallback(
      (state) => (sessionId ? state.getSession(sessionId)?.pipeline : undefined),
      [sessionId]
    )
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
    pipeline: pipeline ?? null,
    isConnected: isConnectedFromStore,
    tuneNode,
    tuneNodeConfig,
    addNode,
    removeNode,
    connectPins,
    disconnectPins,
    applyBatch,
  };
}
