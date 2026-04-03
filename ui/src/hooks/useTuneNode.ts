// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useCallback } from 'react';
import { v4 as uuidv4 } from 'uuid';

import { getWebSocketService } from '@/services/websocket';
import { writeNodeParams } from '@/stores/sessionAtoms';
import type { Request, MessageType } from '@/types/types';

/**
 * Lightweight hook that only provides `tuneNodeConfig` without subscribing
 * to pipeline or connection state.  Use this in components that need to
 * send `UpdateParams` messages but don't need to read session state (e.g.
 * `OverlayControls`).  This avoids unnecessary re-renders caused by the
 * broader `useSession` hook's subscriptions.
 */
export function useTuneNode(sessionId: string | null) {
  const wsService = getWebSocketService();

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

  return { tuneNodeConfig };
}
