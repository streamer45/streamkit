// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Hook for reading per-node state from Jotai atoms.
 *
 * Node components use this instead of reading `state` from ReactFlow's
 * `data` prop, so that state transitions re-render only the affected
 * node — not every node on the canvas.
 *
 * Params are deliberately NOT read from atoms in node components.
 * Individual slider/toggle/text controls subscribe to the params atom
 * directly via `useNumericSlider` / `useTuneNode`, which confines
 * re-renders to just the control that changed — rather than the entire
 * node subtree on every drag tick.
 *
 * When `sessionId` is available (monitor view), the hook subscribes to
 * the per-node atom family.  When absent (design view), it falls back to
 * a static null atom and returns the caller-supplied fallback value.
 */

import { useAtomValue } from 'jotai/react';

import { nodeKey, nodeStateAtom, nullStateAtom } from '@/stores/sessionAtoms';
import type { NodeState } from '@/types/types';

/**
 * Read a node's `NodeState` from the per-node Jotai atom.
 *
 * @param nodeId   ReactFlow node id
 * @param sessionId  Active session id (undefined in design view)
 * @param fallback   Value to return when the atom is empty or sessionId is absent
 */
export function useNodeStateFromAtom(
  nodeId: string,
  sessionId: string | undefined,
  fallback?: NodeState
): NodeState | undefined {
  const key = sessionId ? nodeKey(sessionId, nodeId) : null;
  const atomState = useAtomValue(key ? nodeStateAtom(key) : nullStateAtom);
  return (key ? atomState : null) ?? fallback ?? undefined;
}
