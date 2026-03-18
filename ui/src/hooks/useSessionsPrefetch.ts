// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useQuery } from '@tanstack/react-query';
import { useEffect } from 'react';

import { fetchApi } from '@/services/base';
import {
  sessionStore as defaultSessionStore,
  nodeStateAtom,
  nodeViewDataAtom,
  nodeKey,
} from '@/stores/sessionAtoms';
import { useSessionStore } from '@/stores/sessionStore';
import type { Pipeline, SessionInfo } from '@/types/types';

async function fetchPipeline(sessionId: string): Promise<Pipeline> {
  const response = await fetchApi(`/api/v1/sessions/${sessionId}/pipeline`);
  if (!response.ok) {
    throw new Error(`Failed to fetch pipeline: ${response.statusText}`);
  }
  return response.json();
}

/** Seed Jotai atoms with initial node states and view data from pipelines. */
function seedSessionAtoms(batch: Array<{ sessionId: string; pipeline: Pipeline }>): void {
  for (const { sessionId, pipeline } of batch) {
    if (pipeline.nodes) {
      for (const [nodeId, node] of Object.entries(pipeline.nodes)) {
        if (node.state) {
          defaultSessionStore.set(nodeStateAtom(nodeKey(sessionId, nodeId)), node.state);
        }
      }
    }
    if (pipeline.view_data && typeof pipeline.view_data === 'object') {
      for (const [nodeId, data] of Object.entries(pipeline.view_data as Record<string, unknown>)) {
        defaultSessionStore.set(nodeViewDataAtom(nodeKey(sessionId, nodeId)), data);
      }
    }
  }
}

/**
 * Prefetch pipeline data for all sessions to enable status display
 * without requiring the session to be selected first
 */
export function useSessionsPrefetch(sessions: SessionInfo[]) {
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

  // Batch-update Zustand store when pipeline data is fetched.
  // Using batchSetPipelines applies all pipeline updates in a single
  // set() call, avoiding N individual Map recreations and N subscriber
  // notifications that previously caused render cascades.
  const batchSetPipelines = useSessionStore((state) => state.batchSetPipelines);
  useEffect(() => {
    if (queries.data) {
      const batch = queries.data
        .filter(
          (entry): entry is { sessionId: string; pipeline: Pipeline } => entry.pipeline !== null
        )
        .map(({ sessionId, pipeline }) => ({ sessionId, pipeline }));

      if (batch.length > 0) {
        batchSetPipelines(batch);
        seedSessionAtoms(batch);
      }
    }
  }, [queries.data, batchSetPipelines]);

  return queries;
}
