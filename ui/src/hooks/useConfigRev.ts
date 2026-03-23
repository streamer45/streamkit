// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Per-node config revision counter for causal consistency.
 *
 * Each node's outgoing config carries a monotonically increasing `_rev`
 * alongside the WebSocket session's `_sender` nonce.  Consumers
 * (handleNodeParamsChanged, useServerLayoutSync) compare the incoming
 * `(_sender, _rev)` against the local counter to detect and discard
 * stale self-echoes.
 *
 * The counter is per-node because different nodes may be edited at
 * different rates.  The `_sender` nonce comes from WebSocketService
 * and is stable for one WS connection lifetime.
 */

import { getWebSocketService } from '@/services/websocket';

// ── Singleton rev counters ──────────────────────────────────────────────────

/** Per-node config revision counters, keyed by nodeId.
 *  Shared across all hook instances — a ref-map so React doesn't
 *  re-render when the counter bumps. */
const nodeRevCounters = new Map<string, number>();

/** Get the current local config rev for a node (non-reactive). */
export function getLocalConfigRev(nodeId: string): number {
  return nodeRevCounters.get(nodeId) ?? 0;
}

/** Bump and return the new config rev for a node. */
export function bumpConfigRev(nodeId: string): number {
  const next = (nodeRevCounters.get(nodeId) ?? 0) + 1;
  nodeRevCounters.set(nodeId, next);
  return next;
}

/** Reset all config rev counters (e.g. on WS reconnect). */
export function resetAllConfigRevs(): void {
  nodeRevCounters.clear();
}

/** Get the current client nonce from the WebSocket service. */
export function getClientNonce(): string {
  return getWebSocketService().getClientNonce();
}
