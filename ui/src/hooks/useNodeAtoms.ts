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
 * Most controls (sliders, toggles, text inputs) subscribe to the params
 * atom directly via `useNumericSlider` / `useTuneNode`, which confines
 * re-renders to just the control that changed.  Compositor layers are
 * an exception — they receive params as a prop from the node component
 * and sync remote changes via `compositorServerSync` (view data) rather
 * than the params atom.  See #398 for improving this.
 *
 * `sessionId` is stable for the lifetime of a mount — design view always
 * passes `undefined`, monitor view always passes a string.  The
 * `nullStateAtom` branch exists to satisfy the rules of hooks (always
 * call `useAtomValue`), not because the value toggles at runtime.
 */

import { useAtomValue } from 'jotai/react';

import { nodeKey, nodeStateAtom, nullStateAtom } from '@/stores/sessionAtoms';
import type { NodeState } from '@/types/types';

/**
 * Read a node's `NodeState` from the per-node Jotai atom.
 *
 * @param nodeId   ReactFlow node id
 * @param sessionId  Active session id (undefined in design view)
 * @param fallback   Value to return when sessionId is absent (design view)
 */
export function useNodeStateFromAtom(
  nodeId: string,
  sessionId: string | undefined,
  fallback?: NodeState
): NodeState | undefined {
  const key = sessionId ? nodeKey(sessionId, nodeId) : null;
  const atomState = useAtomValue(key ? nodeStateAtom(key) : nullStateAtom);
  // Keyed branch: return atom value directly (null → undefined).
  // Unkeyed branch (design view): return fallback.
  if (key) return atomState ?? undefined;
  return fallback ?? undefined;
}
