// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useRef } from 'react';

import { listSessions } from '@/services/sessions';
import type { Event, SessionInfo } from '@/types/types';
import { hooksLogger } from '@/utils/logger';

import { useWebSocket } from './useWebSocket';

export function useSessionList() {
  const queryClient = useQueryClient();
  const { onMessage } = useWebSocket();
  const destroyTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    const unsubscribe = onMessage((message) => {
      if (message.type === 'event') {
        const event = message as Event;
        if (event.payload.event === 'sessioncreated') {
          hooksLogger.debug(
            'useSessionList: Invalidating sessions query due to WebSocket event:',
            event.payload.event
          );
          queryClient.invalidateQueries({ queryKey: ['sessions'] });
        } else if (event.payload.event === 'sessiondestroyed') {
          const destroyedId = event.payload.session_id;
          hooksLogger.debug(
            'useSessionList: Optimistically removing destroyed session:',
            destroyedId
          );
          // Optimistically remove the destroyed session from the cache
          // immediately so the UI never flickers.  A deferred invalidation
          // re-fetches the list once the backend has fully cleaned up.
          queryClient.setQueryData<SessionInfo[]>(['sessions'], (old) =>
            old?.filter((s) => s.id !== destroyedId)
          );
          // Defer the refetch so the HTTP response arrives after the
          // backend has actually removed the session from its map.
          if (destroyTimerRef.current) {
            clearTimeout(destroyTimerRef.current);
          }
          destroyTimerRef.current = setTimeout(() => {
            destroyTimerRef.current = null;
            queryClient.invalidateQueries({ queryKey: ['sessions'] });
          }, 2000);
        }
      }
    });

    return () => {
      unsubscribe();
      if (destroyTimerRef.current) {
        clearTimeout(destroyTimerRef.current);
      }
    };
  }, [onMessage, queryClient]);

  return useQuery({
    queryKey: ['sessions'],
    queryFn: listSessions,
    refetchInterval: 10000, // Poll every 10 seconds as fallback (WebSocket is primary)
    staleTime: 5000, // Consider data fresh for 5 seconds
    refetchOnWindowFocus: true, // Refetch when user returns to the tab
    refetchOnMount: 'always', // Always refetch when component mounts (entering Monitor view)
  });
}
