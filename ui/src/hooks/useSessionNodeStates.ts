// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useCallback, useMemo, useSyncExternalStore } from 'react';

import { sessionStore, nodeStateAtom, nodeKey } from '@/stores/sessionAtoms';
import { useSessionStore } from '@/stores/sessionStore';
import type { NodeState, Pipeline } from '@/types/types';

function shallowEqualNodeStates(
  a: Record<string, NodeState>,
  b: Record<string, NodeState>
): boolean {
  const aKeys = Object.keys(a);
  const bKeys = Object.keys(b);
  if (aKeys.length !== bKeys.length) return false;
  for (const key of aKeys) {
    if (a[key] !== b[key]) return false;
  }
  return true;
}

// Lives outside the hook so the snapshot closure mutation stays opaque to
// the React Compiler.
function createNodeStatesStore(sessionId: string, pipeline: Pipeline | null) {
  const nodeIds = pipeline ? Object.keys(pipeline.nodes) : [];

  const readAll = (): Record<string, NodeState> => {
    const states: Record<string, NodeState> = {};
    for (const id of nodeIds) {
      const state = sessionStore.get(nodeStateAtom(nodeKey(sessionId, id)));
      if (state) states[id] = state;
    }
    return states;
  };

  let snapshot = readAll();

  const subscribe = (onStoreChange: () => void) => {
    let disposed = false;
    let pending = false;
    const onAtomChange = () => {
      if (pending) return;
      pending = true;
      queueMicrotask(() => {
        pending = false;
        if (disposed) return;
        const next = readAll();
        if (!shallowEqualNodeStates(snapshot, next)) {
          snapshot = next;
          onStoreChange();
        }
      });
    };

    const unsubs = nodeIds.map((id) =>
      sessionStore.sub(nodeStateAtom(nodeKey(sessionId, id)), onAtomChange)
    );

    // Catch atom writes that landed between snapshot creation (render)
    // and subscription (commit).
    onAtomChange();

    return () => {
      disposed = true;
      unsubs.forEach((u) => u());
    };
  };

  return { subscribe, getSnapshot: () => snapshot };
}

/**
 * Read all node states for a session from per-node Jotai atoms.
 *
 * Uses the pipeline from the Zustand session store to discover node IDs,
 * then subscribes to each node's Jotai state atom for live updates.
 * A shallow-equality guard prevents unnecessary re-renders when atom
 * values haven't changed (e.g. duplicate writes within a single RAF flush).
 */
export function useSessionNodeStates(sessionId: string): Record<string, NodeState> {
  const pipeline = useSessionStore(
    useCallback((state) => state.getSession(sessionId)?.pipeline ?? null, [sessionId])
  );

  const store = useMemo(() => createNodeStatesStore(sessionId, pipeline), [sessionId, pipeline]);

  return useSyncExternalStore(store.subscribe, store.getSnapshot);
}
