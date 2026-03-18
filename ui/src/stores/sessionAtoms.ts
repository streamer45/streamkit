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

import type { NodeState, NodeStats } from '@/types/types';

// ── Default store reference ─────────────────────────────────────────────────

/** The default (provider-less) Jotai store for session atoms.
 *  Used by the WebSocket service (non-React) and compositorServerSync
 *  (inside compositor Provider where the default store isn't reachable
 *  via useAtomValue). */
export const sessionStore = getDefaultStore();

// ── Composite key helper ────────────────────────────────────────────────────

export function nodeKey(sessionId: string, nodeId: string): string {
  return `${sessionId}\0${nodeId}`;
}

/** Static atom that always returns null.  Used as a subscription target when
 *  a component needs to conditionally opt out of a high-frequency atom
 *  (e.g. stats when the tooltip is closed) without breaking the rules of hooks. */
export const nullStatsAtom = atom<NodeStats | null>(null);

// ── Per-node atoms ──────────────────────────────────────────────────────────

export const nodeStateAtom = atomFamily(
  (_key: string) => atom<NodeState | null>(null) // eslint-disable-line @typescript-eslint/no-unused-vars
);

export const nodeStatsAtom = atomFamily(
  (_key: string) => atom<NodeStats | null>(null) // eslint-disable-line @typescript-eslint/no-unused-vars
);

export const nodeViewDataAtom = atomFamily(
  (_key: string) => atom<unknown>(undefined) // eslint-disable-line @typescript-eslint/no-unused-vars
);

// ── Per-node params atom ────────────────────────────────────────────────────

/** Per-node params atom -- stores the full Record<string, unknown> for a node. */
export const nodeParamsAtom = atomFamily(
  (_key: string) => atom<Record<string, unknown>>({}) // eslint-disable-line @typescript-eslint/no-unused-vars
);

/** Write a single node param to the Jotai atom. */
export function writeNodeParam(
  nodeId: string,
  key: string,
  value: unknown,
  sessionId?: string
): void {
  const k = sessionId ? `${sessionId}\0${nodeId}` : nodeId;
  const current = sessionStore.get(nodeParamsAtom(k));
  sessionStore.set(nodeParamsAtom(k), { ...current, [key]: value });
}

/** Write multiple node params to the Jotai atom. */
export function writeNodeParams(
  nodeId: string,
  params: Record<string, unknown>,
  sessionId?: string
): void {
  const k = sessionId ? `${sessionId}\0${nodeId}` : nodeId;
  const current = sessionStore.get(nodeParamsAtom(k));
  sessionStore.set(nodeParamsAtom(k), { ...current, ...params });
}

/** Clear node params atom for a specific node. */
export function clearNodeParams(nodeId: string, sessionId?: string): void {
  const k = sessionId ? `${sessionId}\0${nodeId}` : nodeId;
  sessionStore.set(nodeParamsAtom(k), {});
  nodeParamsAtom.remove(k);
}

// ── Per-session connected atom ──────────────────────────────────────────────

export const sessionConnectedAtom = atomFamily(
  (_sessionId: string) => atom(false) // eslint-disable-line @typescript-eslint/no-unused-vars
);

// ── Batch write helpers ─────────────────────────────────────────────────────

/** Write batched node state updates to atoms. Called from WebSocket RAF flush. */
export function batchWriteNodeStates(updates: Map<string, Record<string, NodeState>>): void {
  for (const [sessionId, nodeUpdates] of updates) {
    for (const [nodeId, state] of Object.entries(nodeUpdates)) {
      sessionStore.set(nodeStateAtom(nodeKey(sessionId, nodeId)), state);
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

/** Clear all atoms for a session (on session destroy).
 *  Iterates atomFamily caches (jotai-family supports iteration) and removes
 *  entries whose key starts with the session prefix to prevent memory leaks. */
export function clearSessionAtoms(sessionId: string): void {
  const prefix = `${sessionId}\0`;

  for (const key of nodeStateAtom.getParams()) {
    if (key.startsWith(prefix)) {
      sessionStore.set(nodeStateAtom(key), null);
      nodeStateAtom.remove(key);
    }
  }
  for (const key of nodeStatsAtom.getParams()) {
    if (key.startsWith(prefix)) {
      sessionStore.set(nodeStatsAtom(key), null);
      nodeStatsAtom.remove(key);
    }
  }
  for (const key of nodeViewDataAtom.getParams()) {
    if (key.startsWith(prefix)) {
      sessionStore.set(nodeViewDataAtom(key), undefined);
      nodeViewDataAtom.remove(key);
    }
  }
  for (const key of nodeParamsAtom.getParams()) {
    if (key.startsWith(prefix)) {
      sessionStore.set(nodeParamsAtom(key), {});
      nodeParamsAtom.remove(key);
    }
  }

  sessionStore.set(sessionConnectedAtom(sessionId), false);
  sessionConnectedAtom.remove(sessionId);
}
