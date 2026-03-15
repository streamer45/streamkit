// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Hook that bridges Zustand node-state updates to ReactFlow nodes/edges
 * without re-rendering MonitorViewContent.
 *
 * Instead of subscribing reactively to `nodeStates` (which would re-render
 * the entire component on every node-state transition), this hook subscribes
 * directly to the Zustand store and patches ReactFlow nodes/edges from the
 * callback.  This completely bypasses React's render cycle for high-frequency
 * state changes during session load.
 *
 * Patches are throttled: the first change applies immediately, then
 * subsequent changes within PATCH_THROTTLE_MS are coalesced into a single
 * deferred patch.  During session load, ~8 node-state transitions that
 * would each trigger a full ~20 ms MonitorViewContent re-render are
 * collapsed into 2–3 patches instead.
 */

import React, { useEffect, useRef } from 'react';
import type { Node as RFNode, Edge } from '@xyflow/react';

import { useSessionStore } from '@/stores/sessionStore';
import type { NodeState, Pipeline } from '@/types/types';
import { deepEqual } from '@/utils/deepEqual';
import {
  isRecord,
  extractSlowTimeoutDetailsFromNodeState,
  describeSlowInputs,
  type SlowTimeoutDetails,
} from '@/utils/pipelineGraph';

const EMPTY_PARAMS: Record<string, unknown> = {};

export interface UseNodeStatesSubscriptionOptions {
  selectedSessionId: string | null;
  setNodes: React.Dispatch<React.SetStateAction<RFNode[]>>;
  setEdges: React.Dispatch<React.SetStateAction<Edge[]>>;
  pipelineRef: React.RefObject<Pipeline | undefined | null>;
  topoKey: string;
}

export interface UseNodeStatesSubscriptionReturn {
  /** Set to `true` by the topology effect after building the initial graph. */
  topoEffectRanRef: React.MutableRefObject<boolean>;
}

export function useNodeStatesSubscription({
  selectedSessionId,
  setNodes,
  setEdges,
  pipelineRef,
  topoKey,
}: UseNodeStatesSubscriptionOptions): UseNodeStatesSubscriptionReturn {
  // Track previous topoKey to avoid redundant patch effect when topology changes
  const prevTopoKeyRef = useRef<string>('');
  const topoEffectRanRef = useRef(false);
  const isInitialMountRef = useRef(true);

  // Keep topoKey accessible from the store subscription without stale closures
  const topoKeyRef = useRef(topoKey);
  topoKeyRef.current = topoKey;

  useEffect(() => {
    if (!selectedSessionId) return;

    const PATCH_THROTTLE_MS = 100;

    let prevNodeStates: Record<string, NodeState> | undefined;
    let lastPatchTime = 0;
    let throttleTimer: ReturnType<typeof setTimeout> | null = null;
    let pendingNodeStates: Record<string, NodeState> | null = null;
    isInitialMountRef.current = true;

    const applyPatch = (nodeStates: Record<string, NodeState>) => {
      lastPatchTime = performance.now();

      const currentPipeline = pipelineRef.current;
      if (!currentPipeline) return;

      // ── Patch nodes + edges in one transition to avoid double render ──
      React.startTransition(() => {
        // Patch node state/params
        setNodes((prev) => {
          const updatesById = new Map<
            string,
            { nextState: unknown; nextParams: Record<string, unknown> }
          >();

          for (const n of prev) {
            const apiNode = currentPipeline.nodes[n.id];
            if (!apiNode) continue;

            const nextState = nodeStates[n.id] ?? apiNode.state;
            const nextParams: Record<string, unknown> =
              apiNode.params && typeof apiNode.params === 'object' && !Array.isArray(apiNode.params)
                ? (apiNode.params as Record<string, unknown>)
                : EMPTY_PARAMS;

            const stateChanged = !deepEqual(n.data.state, nextState);
            const paramsChanged = !deepEqual(n.data.params, nextParams);

            if (stateChanged || paramsChanged) {
              updatesById.set(n.id, { nextState, nextParams });
            }
          }

          if (updatesById.size === 0) return prev;

          return prev.map((n) => {
            const updateInfo = updatesById.get(n.id);
            if (!updateInfo) return n;
            return {
              ...n,
              data: {
                ...n.data,
                state: updateInfo.nextState,
                params: updateInfo.nextParams,
              },
            };
          });
        });

        // Patch edge alerts (slow-input-timeout)
        const slowPinsByNode = new Map<string, Set<string>>();
        const slowDetailsByNode = new Map<string, SlowTimeoutDetails>();
        for (const [nodeId, apiNode] of Object.entries(currentPipeline.nodes)) {
          const st = (nodeStates as Record<string, NodeState>)[nodeId] ?? apiNode.state ?? null;
          const details = extractSlowTimeoutDetailsFromNodeState(st);
          const slowPins = details?.slowPins ?? [];
          if (slowPins.length > 0) {
            slowPinsByNode.set(nodeId, new Set(slowPins));
          }
          if (details) {
            slowDetailsByNode.set(nodeId, details);
          }
        }

        setEdges((prev) => {
          let changed = false;

          const next = prev.map((edge) => {
            const targetPin = edge.targetHandle ?? '';
            const shouldWarn = slowPinsByNode.get(edge.target)?.has(targetPin) ?? false;
            const currentAlert = isRecord(edge.data) ? edge.data['alert'] : undefined;
            const currentAlertKind =
              isRecord(currentAlert) && typeof currentAlert['kind'] === 'string'
                ? currentAlert['kind']
                : null;
            const isCurrentlyWarned = currentAlertKind === 'slow_input_timeout';

            if (shouldWarn === isCurrentlyWarned) return edge;

            changed = true;
            const nextData: Record<string, unknown> = { ...(edge.data || {}) };

            if (shouldWarn) {
              const details = slowDetailsByNode.get(edge.target);
              const slowPins = details?.slowPins ?? [];
              const p = pipelineRef.current;
              const slowInputs = p ? describeSlowInputs(p, edge.target, slowPins) : [];

              const lines: string[] = [];
              if (slowInputs.length > 0) {
                lines.push(`Slow inputs: ${slowInputs.join(', ')}`);
              } else if (slowPins.length > 0) {
                lines.push(`Slow pins: ${slowPins.join(', ')}`);
              }

              const sourceHandle = edge.sourceHandle ?? '';
              lines.push(`This: ${edge.source}.${sourceHandle} → ${edge.targetHandle ?? ''}`);

              if (details?.newlySlowPins && details.newlySlowPins.length > 0) {
                lines.push(`Newly slow: ${details.newlySlowPins.join(', ')}`);
              }
              if (details?.syncTimeoutMs !== null && details?.syncTimeoutMs !== undefined) {
                lines.push(`Timeout: ${details.syncTimeoutMs}ms`);
              }

              nextData.alert = {
                kind: 'slow_input_timeout',
                severity: 'warning',
                tooltip: {
                  title: `${edge.target} degraded`,
                  lines,
                },
              };
            } else if (isCurrentlyWarned) {
              delete nextData.alert;
            }

            return { ...edge, data: nextData };
          });

          return changed ? next : prev;
        });
      });
    };

    const unsubscribe = useSessionStore.subscribe((state) => {
      const session = state.sessions.get(selectedSessionId);
      const nodeStates = session?.nodeStates;

      // Skip if same reference (store changed for a different reason)
      if (nodeStates === prevNodeStates) return;
      prevNodeStates = nodeStates;

      // Skip on initial mount — let the topology effect handle everything
      if (isInitialMountRef.current) {
        isInitialMountRef.current = false;
        prevTopoKeyRef.current = topoKeyRef.current;
        return;
      }

      // If topoKey changed, the topology effect will handle the full rebuild
      if (prevTopoKeyRef.current !== topoKeyRef.current) {
        prevTopoKeyRef.current = topoKeyRef.current;
        return;
      }

      // Don't patch until the topology effect has built the initial graph
      if (!topoEffectRanRef.current) return;

      if (!nodeStates) return;

      // ── Throttled patch ────────────────────────────────────────────────
      // Apply immediately if enough time elapsed since the last patch;
      // otherwise buffer and apply after the throttle window.  During
      // session-load bursts this collapses ~8 individual setNodes calls
      // (each triggering a ~20 ms MonitorViewContent re-render) into 2–3.
      pendingNodeStates = nodeStates;
      const now = performance.now();
      const elapsed = now - lastPatchTime;

      if (elapsed >= PATCH_THROTTLE_MS) {
        // First change or enough time since last patch — apply now.
        if (throttleTimer !== null) {
          clearTimeout(throttleTimer);
          throttleTimer = null;
        }
        applyPatch(nodeStates);
      } else if (throttleTimer === null) {
        // Schedule a trailing-edge flush.
        throttleTimer = setTimeout(() => {
          throttleTimer = null;
          if (pendingNodeStates) {
            applyPatch(pendingNodeStates);
            pendingNodeStates = null;
          }
        }, PATCH_THROTTLE_MS - elapsed);
      }
    });

    return () => {
      unsubscribe();
      if (throttleTimer !== null) clearTimeout(throttleTimer);
    };
  }, [selectedSessionId, setNodes, setEdges, pipelineRef]);

  return { topoEffectRanRef };
}
