// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Hooks for reading per-node state and params from Jotai atoms.
 *
 * Node components use these instead of reading from ReactFlow's `data` prop,
 * so that state/params changes re-render only the affected node — not every
 * node on the canvas.
 *
 * When `sessionId` is available (monitor view), the hooks subscribe to the
 * per-node atom family.  When absent (design view), they fall back to a
 * static null atom and return the caller-supplied fallback value.
 */

import { useAtomValue } from 'jotai/react';
import { useMemo } from 'react';

import {
  nodeKey,
  nodeParamsAtom,
  nodeStateAtom,
  nullParamsAtom,
  nullStateAtom,
} from '@/stores/sessionAtoms';
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

/**
 * Read a node's params from the per-node Jotai atom, merged with pipeline
 * defaults.
 *
 * The atom accumulates runtime param updates from WebSocket events.  On
 * initial render the atom may still be empty `{}`, so we merge with
 * `fallback` (the pipeline-definition params) to ensure default values
 * are present.  Atom values take precedence over fallback.
 *
 * @param nodeId    ReactFlow node id
 * @param sessionId Active session id (undefined in design view)
 * @param fallback  Pipeline-definition params (`data.params`)
 */
export function useNodeParamsFromAtom(
  nodeId: string,
  sessionId: string | undefined,
  fallback: Record<string, unknown>
): Record<string, unknown> {
  const key = sessionId ? nodeKey(sessionId, nodeId) : null;
  const atomParams = useAtomValue(key ? nodeParamsAtom(key) : nullParamsAtom);
  return useMemo(() => {
    if (!key || Object.keys(atomParams).length === 0) return fallback;
    return { ...fallback, ...atomParams };
  }, [key, fallback, atomParams]);
}
