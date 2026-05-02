// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Hook that bridges Zustand node-state updates to ReactFlow edges
 * without re-rendering MonitorViewContent.
 *
 * Node components read their `state` directly from per-node Jotai atoms
 * (via {@link useNodeStateFromAtom}), so this hook no longer patches
 * ReactFlow node data for state.  Params are NOT read from atoms in node
 * components — individual controls (sliders, toggles) subscribe to the
 * params atom directly via `useNumericSlider` / `useTuneNode`, which
 * confines re-renders to just the affected control.
 *
 * This hook still subscribes to the Zustand store to patch edge alert
 * metadata (slow-input-timeout warnings).
 *
 * Patches are throttled: the first change applies immediately, then
 * subsequent changes within PATCH_THROTTLE_MS are coalesced into a single
 * deferred patch.
 */

import type { Edge } from '@xyflow/react';
import React, { useEffect, useRef } from 'react';

import { useSessionStore } from '@/stores/sessionStore';
import type { NodeState, Pipeline } from '@/types/types';
import {
  isRecord,
  extractSlowTimeoutDetailsFromNodeState,
  describeSlowInputs,
  type SlowTimeoutDetails,
} from '@/utils/pipelineGraph';

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

      // Patch edge alerts (slow-input-timeout)
      React.startTransition(() => {
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
      // otherwise buffer and apply after the throttle window.
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
  }, [selectedSessionId, setEdges, pipelineRef]);

  return { topoEffectRanRef };
}
