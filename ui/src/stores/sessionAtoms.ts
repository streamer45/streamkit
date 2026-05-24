// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Jotai atoms for high-frequency session data (node states, stats, view data).
 *
 * These atoms live in the default (provider-less) store and are written by
 * the WebSocket service's RAF-batched flush.  Per-node atoms confine
 * re-renders to the affected node's components — a state change on node A
 * doesn't wake up components reading node B's state.
 *
 * Pipeline and connection management remain in the Zustand sessionStore
 * (low-frequency CRUD operations that work well with Zustand's patterns).
 */

import { atom, getDefaultStore } from 'jotai';
import { atomFamily } from 'jotai-family';

import type { NodeState, NodeStats, Pipeline } from '@/types/types';
import { deepMerge } from '@/utils/controlProps';
import { deepEqual } from '@/utils/deepEqual';

/** The default (provider-less) Jotai store for session atoms.
 *  Used by the WebSocket service (non-React) and compositorServerSync
 *  (inside compositor Provider where the default store isn't reachable
 *  via useAtomValue). */
export const sessionStore = getDefaultStore();

export function nodeKey(sessionId: string, nodeId: string): string {
  return `${sessionId}\0${nodeId}`;
}

/** Static atom that always returns null.  Used as a subscription target when
 *  a component needs to conditionally opt out of a high-frequency atom
 *  (e.g. stats when the tooltip is closed) without breaking the rules of hooks. */
export const nullStatsAtom = atom<NodeStats | null>(null);

/** Static atom that always returns null.  Used when a node component has no
 *  `sessionId` (e.g. design view) to avoid creating a permanent empty-key
 *  entry in `nodeStateAtom`. */
export const nullStateAtom = atom<NodeState | null>(null);

/** Static atom that always returns false.  Used when `sessionId` is null to
 *  avoid creating a permanent empty-key entry in `sessionConnectedAtom`. */
export const nullConnectedAtom = atom(false);

export const nodeStateAtom = atomFamily((_key: string) => atom<NodeState | null>(null));
export const nodeStatsAtom = atomFamily((_key: string) => atom<NodeStats | null>(null));
export const nodeViewDataAtom = atomFamily((_key: string) => atom<unknown>(undefined));

/** Per-node params atom -- stores the full Record<string, unknown> for a node. */
export const nodeParamsAtom = atomFamily((_key: string) => atom<Record<string, unknown>>({}));

/** Write a single flat-key node param to the Jotai atom.
 *  This performs a shallow merge — suitable for top-level scalar keys only
 *  (e.g. `gain_db`).  For nested/dot-path updates, use `writeNodeParams`
 *  which deep-merges to preserve sibling properties. */
export function writeNodeParam(
  nodeId: string,
  key: string,
  value: unknown,
  sessionId?: string
): void {
  const k = sessionId ? `${sessionId}\0${nodeId}` : nodeId;
  const current = sessionStore.get(nodeParamsAtom(k));
  if (current[key] === value) return;
  sessionStore.set(nodeParamsAtom(k), { ...current, [key]: value });
}

/** Write multiple node params to the Jotai atom.
 *  Transient sync metadata (`_sender`, `_rev`, etc.) is stripped
 *  so it doesn't pollute the local store. */
export function writeNodeParams(
  nodeId: string,
  params: Record<string, unknown>,
  sessionId?: string
): void {
  const k = sessionId ? `${sessionId}\0${nodeId}` : nodeId;
  const current = sessionStore.get(nodeParamsAtom(k));
  const cleaned: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(params)) {
    if (!key.startsWith('_')) {
      cleaned[key] = value;
    }
  }
  const merged = deepMerge(current, cleaned);
  if (!deepEqual(current, merged)) {
    sessionStore.set(nodeParamsAtom(k), merged);
  }
}

/** Clear node params atom for a specific node. */
export function clearNodeParams(nodeId: string, sessionId?: string): void {
  const k = sessionId ? `${sessionId}\0${nodeId}` : nodeId;
  nodeParamsAtom.remove(k);
}

/** Reset all node params for a session (e.g. on unsubscribe).
 *  Sets atoms to empty objects but does NOT remove from the cache —
 *  the session may be resubscribed and the atoms reused. */
export function resetSessionParams(sessionId: string): void {
  const prefix = `${sessionId}\0`;
  for (const key of [...nodeParamsAtom.getParams()].filter((k) => k.startsWith(prefix))) {
    sessionStore.set(nodeParamsAtom(key), {});
  }
}

export const sessionConnectedAtom = atomFamily((_sessionId: string) => atom(false));

/** Write batched node state updates to atoms. Called from WebSocket RAF flush.
 *  Skips writes when the new value is deeply equal to the current one so
 *  that Jotai subscribers (node components) are not notified for no-op
 *  state transitions — this is the atom-side equivalent of the deepEqual
 *  guard that the old setNodes() patching path had. */
export function batchWriteNodeStates(updates: Map<string, Record<string, NodeState>>): void {
  for (const [sessionId, nodeUpdates] of updates) {
    for (const [nodeId, state] of Object.entries(nodeUpdates)) {
      const key = nodeKey(sessionId, nodeId);
      const current = sessionStore.get(nodeStateAtom(key));
      if (!deepEqual(current, state)) {
        sessionStore.set(nodeStateAtom(key), state);
      }
    }
  }
}

/** Write batched node stats updates to atoms. Called from WebSocket RAF flush. */
export function batchWriteNodeStats(updates: Map<string, Record<string, NodeStats>>): void {
  for (const [sessionId, nodeUpdates] of updates) {
    for (const [nodeId, stats] of Object.entries(nodeUpdates)) {
      sessionStore.set(nodeStatsAtom(nodeKey(sessionId, nodeId)), stats);
    }
  }
}

/** Write node view data for a specific node. */
export function writeNodeViewData(sessionId: string, nodeId: string, data: unknown): void {
  sessionStore.set(nodeViewDataAtom(nodeKey(sessionId, nodeId)), data);
}

/** Write session connected status. */
export function writeSessionConnected(sessionId: string, connected: boolean): void {
  sessionStore.set(sessionConnectedAtom(sessionId), connected);
}

/** Seed Jotai atoms with initial node states and view data from a pipeline.
 *  Called when pipeline data first arrives (fetch or batch prefetch). */
export function seedPipelineAtoms(sessionId: string, pipeline: Pipeline): void {
  if (pipeline.nodes) {
    for (const [nodeId, node] of Object.entries(pipeline.nodes)) {
      if (node.state) {
        const key = nodeKey(sessionId, nodeId);
        const current = sessionStore.get(nodeStateAtom(key));
        if (!deepEqual(current, node.state)) {
          sessionStore.set(nodeStateAtom(key), node.state);
        }
      }
    }
  }
  if (pipeline.view_data && typeof pipeline.view_data === 'object') {
    for (const [nodeId, data] of Object.entries(pipeline.view_data as Record<string, unknown>)) {
      sessionStore.set(nodeViewDataAtom(nodeKey(sessionId, nodeId)), data);
    }
  }
}

/** Clear all atoms for a session (on session destroy).
 *  Iterates atomFamily caches (jotai-family supports iteration) and removes
 *  entries whose key starts with the session prefix to prevent memory leaks. */
export function clearSessionAtoms(sessionId: string): void {
  const prefix = `${sessionId}\0`;

  // Snapshot keys into arrays before iterating, so that .remove() during
  // the loop doesn't mutate the iterator mid-flight.
  const stateKeys = [...nodeStateAtom.getParams()].filter((k) => k.startsWith(prefix));
  const statsKeys = [...nodeStatsAtom.getParams()].filter((k) => k.startsWith(prefix));
  const viewKeys = [...nodeViewDataAtom.getParams()].filter((k) => k.startsWith(prefix));
  const paramKeys = [...nodeParamsAtom.getParams()].filter((k) => k.startsWith(prefix));

  for (const key of stateKeys) {
    nodeStateAtom.remove(key);
  }
  for (const key of statsKeys) {
    nodeStatsAtom.remove(key);
  }
  for (const key of viewKeys) {
    nodeViewDataAtom.remove(key);
  }
  for (const key of paramKeys) {
    nodeParamsAtom.remove(key);
  }

  sessionStore.set(sessionConnectedAtom(sessionId), false);
  sessionConnectedAtom.remove(sessionId);
}
