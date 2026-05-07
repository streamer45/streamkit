// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { atom, useAtomValue } from 'jotai';
import { useMemo } from 'react';
import { useShallow } from 'zustand/shallow';

import { nodeStateAtom, nodeKey } from '@/stores/sessionAtoms';
import { useSessionStore } from '@/stores/sessionStore';
import type { NodeState } from '@/types/types';

/**
 * Read all node states for a session from Jotai atoms.
 *
 * Node IDs come from the Zustand pipeline (low-frequency structural data);
 * actual states come from per-node Jotai atoms (high-frequency updates).
 * The derived atom re-evaluates only when an individual node state changes.
 */
export function useSessionNodeStates(sessionId: string): Record<string, NodeState> {
  const nodeIds = useSessionStore(
    useShallow((state) => Object.keys(state.sessions.get(sessionId)?.pipeline?.nodes ?? {}).sort())
  );

  const nodeIdsKey = nodeIds.join('\0');

  const aggregateAtom = useMemo(
    () =>
      atom((get) => {
        const result: Record<string, NodeState> = {};
        for (const id of nodeIdsKey.split('\0')) {
          if (!id) continue;
          const state = get(nodeStateAtom(nodeKey(sessionId, id)));
          if (state != null) result[id] = state;
        }
        return result;
      }),
    [sessionId, nodeIdsKey]
  );

  return useAtomValue(aggregateAtom);
}
