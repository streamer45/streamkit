// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useCallback, useEffect, useState } from 'react';

import { sessionStore, nodeStateAtom, nodeKey } from '@/stores/sessionAtoms';
import { useSessionStore } from '@/stores/sessionStore';
import type { NodeState } from '@/types/types';

/**
 * Read all node states for a session from per-node Jotai atoms.
 *
 * Uses the pipeline from the Zustand session store to discover node IDs,
 * then subscribes to each node's Jotai state atom for live updates.
 */
export function useSessionNodeStates(sessionId: string): Record<string, NodeState> {
  const pipeline = useSessionStore(
    useCallback((state) => state.getSession(sessionId)?.pipeline ?? null, [sessionId])
  );

  const [nodeStates, setNodeStates] = useState<Record<string, NodeState>>({});

  useEffect(() => {
    if (!pipeline) {
      setNodeStates({});
      return;
    }

    const nodeIds = Object.keys(pipeline.nodes);

    const readAll = () => {
      const states: Record<string, NodeState> = {};
      for (const id of nodeIds) {
        const state = sessionStore.get(nodeStateAtom(nodeKey(sessionId, id)));
        if (state) states[id] = state;
      }
      setNodeStates(states);
    };

    readAll();

    let pending = false;
    const onAtomChange = () => {
      if (pending) return;
      pending = true;
      queueMicrotask(() => {
        pending = false;
        readAll();
      });
    };

    const unsubs = nodeIds.map((id) =>
      sessionStore.sub(nodeStateAtom(nodeKey(sessionId, id)), onAtomChange)
    );

    return () => unsubs.forEach((u) => u());
  }, [sessionId, pipeline]);

  return nodeStates;
}
