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

import type { Node as RFNode, Edge } from '@xyflow/react';
import React, { useEffect, useRef } from 'react';

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

/**
 * Shallow-compare two plain data objects field-by-field (===).
 * Returns true when every own-key in both objects is reference-equal,
 * meaning a new wrapper object would be identical to the existing one
 * and the old reference can be preserved.
 *
 * The comparison is symmetric: both key sets are checked so that extra
 * keys in either object cause a mismatch.
 */
function shallowDataEqual(a: Record<string, unknown>, b: Record<string, unknown>): boolean {
  const aKeys = Object.keys(a);
  const bKeys = Object.keys(b);
  if (aKeys.length !== bKeys.length) return false;
  for (const key of aKeys) {
    if (!Object.prototype.hasOwnProperty.call(b, key) || a[key] !== b[key]) return false;
  }
  return true;
}

/** Build tooltip lines for a slow-input-timeout alert. */
function buildSlowInputTooltipLines(
  edge: Edge,
  details: SlowTimeoutDetails | undefined,
  pipeline: Pipeline
): string[] {
  const slowPins = details?.slowPins ?? [];
  const slowInputs = describeSlowInputs(pipeline, edge.target, slowPins);

  const lines: string[] = [];
  if (slowInputs.length > 0) {
    lines.push(`Slow inputs: ${slowInputs.join(', ')}`);
  } else if (slowPins.length > 0) {
    lines.push(`Slow pins: ${slowPins.join(', ')}`);
  }

  lines.push(`This: ${edge.source}.${edge.sourceHandle ?? ''} → ${edge.targetHandle ?? ''}`);

  if (details?.newlySlowPins && details.newlySlowPins.length > 0) {
    lines.push(`Newly slow: ${details.newlySlowPins.join(', ')}`);
  }
  if (details?.syncTimeoutMs != null) {
    lines.push(`Timeout: ${details.syncTimeoutMs}ms`);
  }
  return lines;
}

/**
 * Build edge alert data for slow-input-timeout degradation.
 * Extracted from the main subscription callback to reduce complexity.
 */
function buildEdgeAlert(
  edge: Edge,
  slowPinsByNode: Map<string, Set<string>>,
  slowDetailsByNode: Map<string, SlowTimeoutDetails>,
  pipeline: Pipeline
): Record<string, unknown> | null {
  const shouldWarn = slowPinsByNode.get(edge.target)?.has(edge.targetHandle ?? '') ?? false;
  if (!shouldWarn) return null;

  const details = slowDetailsByNode.get(edge.target);
  return {
    kind: 'slow_input_timeout',
    severity: 'warning',
    tooltip: {
      title: `${edge.target} degraded`,
      lines: buildSlowInputTooltipLines(edge, details, pipeline),
    },
  };
}

/**
 * Collect slow-input-timeout data from node states for edge alert patching.
 */
function collectSlowPinData(
  pipeline: Pipeline,
  nodeStates: Record<string, NodeState>
): {
  slowPinsByNode: Map<string, Set<string>>;
  slowDetailsByNode: Map<string, SlowTimeoutDetails>;
} {
  const slowPinsByNode = new Map<string, Set<string>>();
  const slowDetailsByNode = new Map<string, SlowTimeoutDetails>();
  for (const [nodeId, apiNode] of Object.entries(pipeline.nodes)) {
    const st = nodeStates[nodeId] ?? apiNode.state ?? null;
    const details = extractSlowTimeoutDetailsFromNodeState(st);
    const slowPins = details?.slowPins ?? [];
    if (slowPins.length > 0) {
      slowPinsByNode.set(nodeId, new Set(slowPins));
    }
    if (details) {
      slowDetailsByNode.set(nodeId, details);
    }
  }
  return { slowPinsByNode, slowDetailsByNode };
}

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
    // Reset on every resubscribe so the first patch after a session
    // switch is treated as an initial mount (applied immediately,
    // bypassing the throttle).
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
            const candidateData = {
              ...n.data,
              state: updateInfo.nextState,
              params: updateInfo.nextParams,
            };
            // Preserve data reference identity: if every field of the
            // candidate is reference-equal to the existing data, reuse
            // the old object so areNodePropsEqual's `data === data`
            // check passes and the node component skips re-render.
            if (
              shallowDataEqual(
                n.data as Record<string, unknown>,
                candidateData as Record<string, unknown>
              )
            ) {
              return n;
            }
            return { ...n, data: candidateData };
          });
        });

        // Patch edge alerts (slow-input-timeout)
        const { slowPinsByNode, slowDetailsByNode } = collectSlowPinData(
          currentPipeline,
          nodeStates
        );

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
              nextData.alert = buildEdgeAlert(
                edge,
                slowPinsByNode,
                slowDetailsByNode,
                currentPipeline
              );
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
