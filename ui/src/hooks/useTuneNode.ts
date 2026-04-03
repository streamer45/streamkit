// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useCallback } from 'react';
import { v4 as uuidv4 } from 'uuid';

import { getWebSocketService } from '@/services/websocket';
import { sessionStore, nodeParamsAtom, nodeKey, writeNodeParams } from '@/stores/sessionAtoms';
import type { Request, MessageType } from '@/types/types';
import { deepMerge } from '@/utils/controlProps';

// Resolved once at module level — getWebSocketService returns a singleton,
// so hoisting it avoids a new reference on every render and keeps
// tuneNodeConfig's useCallback deps minimal.
const wsService = getWebSocketService();

/**
 * Lightweight hook that only provides `tuneNodeConfig` without subscribing
 * to pipeline or connection state.  Use this in components that need to
 * send `UpdateParams` messages but don't need to read session state (e.g.
 * `OverlayControls`).  This avoids unnecessary re-renders caused by the
 * broader `useSession` hook's subscriptions.
 *
 * Unlike `useSession.tuneNodeConfig`, this deep-merges partial nested
 * configs into the existing atom state so that sibling properties (e.g.
 * `properties.home_score` and `properties.away_score`) are preserved.
 */
export function useTuneNode(sessionId: string | null) {
  const tuneNodeConfig = useCallback(
    (nodeId: string, config: Record<string, unknown>) => {
      if (!sessionId) return;

      // Deep-merge the partial update into the current atom value so
      // sibling nested properties are preserved (e.g. updating
      // properties.home_score doesn't clobber properties.away_score).
      const k = nodeKey(sessionId, nodeId);
      const current = sessionStore.get(nodeParamsAtom(k));
      const merged = deepMerge(current, config);
      writeNodeParams(nodeId, merged, sessionId);

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
    [sessionId]
  );

  return { tuneNodeConfig };
}
