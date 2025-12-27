// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useQuery } from '@tanstack/react-query';
import { useEffect } from 'react';

import { getApiUrl } from '@/services/base';
import { useSessionStore } from '@/stores/sessionStore';
import type { Pipeline, SessionInfo } from '@/types/types';

async function fetchPipeline(sessionId: string): Promise<Pipeline> {
  const apiUrl = getApiUrl();
  const response = await fetch(`${apiUrl}/api/v1/sessions/${sessionId}/pipeline`);
  if (!response.ok) {
    throw new Error(`Failed to fetch pipeline: ${response.statusText}`);
  }
  return response.json();
}

/**
 * Prefetch pipeline data for all sessions to enable status display
 * without requiring the session to be selected first
 */
export function useSessionsPrefetch(sessions: SessionInfo[]) {
  const setPipeline = useSessionStore((state) => state.setPipeline);

  // Fetch pipeline for each session
  const sessionIds = sessions.map((s) => s.id);

  // Create queries for all sessions
  const queries = useQuery({
    queryKey: ['pipelines-prefetch', sessionIds],
    queryFn: async () => {
      // Fetch all pipelines in parallel
      const results = await Promise.allSettled(sessionIds.map((id) => fetchPipeline(id)));

      return results.map((result, index) => ({
        sessionId: sessionIds[index],
        pipeline: result.status === 'fulfilled' ? result.value : null,
      }));
    },
    enabled: sessionIds.length > 0,
    staleTime: 10000, // Cache for 10 seconds
    refetchInterval: 10000, // Refetch every 10 seconds
  });

  // Update Zustand store when pipeline data is fetched
  useEffect(() => {
    if (queries.data) {
      queries.data.forEach(({ sessionId, pipeline }) => {
        if (pipeline) {
          setPipeline(sessionId, pipeline);
        }
      });
    }
  }, [queries.data, setPipeline]);

  return queries;
}
