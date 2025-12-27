// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect } from 'react';

import { listSessions } from '@/services/sessions';
import type { Event } from '@/types/types';
import { hooksLogger } from '@/utils/logger';

import { useWebSocket } from './useWebSocket';

export function useSessionList() {
  const queryClient = useQueryClient();
  const { onMessage } = useWebSocket();

  useEffect(() => {
    const unsubscribe = onMessage((message) => {
      if (message.type === 'event') {
        const event = message as Event;
        if (
          event.payload.event === 'sessioncreated' ||
          event.payload.event === 'sessiondestroyed'
        ) {
          hooksLogger.debug(
            'useSessionList: Invalidating sessions query due to WebSocket event:',
            event.payload.event
          );
          queryClient.invalidateQueries({ queryKey: ['sessions'] });
        }
      }
    });

    return unsubscribe;
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
