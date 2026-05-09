// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { atom, useAtomValue } from 'jotai';
import { useMemo } from 'react';
import { useShallow } from 'zustand/shallow';

import { nodeStateAtom, nodeKey } from '@/stores/sessionAtoms';
import { useSessionStore } from '@/stores/sessionStore';
import type { NodeState } from '@/types/types';

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

/**
 * Read all node states for a session from Jotai atoms.
 *
 * Node IDs come from the Zustand pipeline (low-frequency structural data);
 * actual states come from per-node Jotai atoms (high-frequency updates).
 * The derived atom re-evaluates only when an individual node state changes.
 *
 * A shallow-equality guard ensures that multiple per-node atom writes within
 * a single RAF flush return the same object reference when no values changed,
 * preserving the coalesced notification behavior of the old Zustand path.
 */
export function useSessionNodeStates(sessionId: string): Record<string, NodeState> {
  const nodeIds = useSessionStore(
    useShallow((state) => Object.keys(state.sessions.get(sessionId)?.pipeline?.nodes ?? {}).sort())
  );

  const nodeIdsKey = nodeIds.join('\0');

  const aggregateAtom = useMemo(() => {
    let prev: Record<string, NodeState> = {};
    return atom((get) => {
      const result: Record<string, NodeState> = {};
      for (const id of nodeIdsKey.split('\0')) {
        if (!id) continue;
        const state = get(nodeStateAtom(nodeKey(sessionId, id)));
        if (state != null) result[id] = state;
      }
      if (shallowEqualNodeStates(prev, result)) return prev;
      prev = result;
      return result;
    });
  }, [sessionId, nodeIdsKey]);

  return useAtomValue(aggregateAtom);
}
