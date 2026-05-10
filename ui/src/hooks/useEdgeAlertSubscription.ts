// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Hook that patches ReactFlow edge alert metadata (slow-input-timeout
 * warnings) by subscribing directly to per-node Jotai state atoms.
 *
 * Node components read their `state` from per-node atoms (via
 * {@link useNodeStateFromAtom}); this hook only patches edge `data.alert`
 * so that warning badges appear on affected edges.
 */

import type { Edge } from '@xyflow/react';
import React, { useEffect, useRef } from 'react';

import { sessionStore, nodeStateAtom, nodeKey } from '@/stores/sessionAtoms';
import type { NodeState, Pipeline } from '@/types/types';
import {
  isRecord,
  extractSlowTimeoutDetailsFromNodeState,
  describeSlowInputs,
  type SlowTimeoutDetails,
} from '@/utils/pipelineGraph';

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

export interface UseEdgeAlertSubscriptionOptions {
  selectedSessionId: string | null;
  setEdges: React.Dispatch<React.SetStateAction<Edge[]>>;
  pipelineRef: React.RefObject<Pipeline | undefined | null>;
  topoKey: string;
}

export interface UseEdgeAlertSubscriptionReturn {
  topoEffectRanRef: React.MutableRefObject<boolean>;
}

export function useEdgeAlertSubscription({
  selectedSessionId,
  setEdges,
  pipelineRef,
  topoKey,
}: UseEdgeAlertSubscriptionOptions): UseEdgeAlertSubscriptionReturn {
  const topoEffectRanRef = useRef(false);

  useEffect(() => {
    if (!selectedSessionId) return;

    topoEffectRanRef.current = false;

    const pipeline = pipelineRef.current;
    const nodeIds = pipeline ? Object.keys(pipeline.nodes) : [];

    const applyPatch = () => {
      const currentPipeline = pipelineRef.current;
      if (!currentPipeline) return;

      const nodeStates: Record<string, NodeState> = {};
      for (const nodeId of Object.keys(currentPipeline.nodes)) {
        const state = sessionStore.get(nodeStateAtom(nodeKey(selectedSessionId, nodeId)));
        if (state) nodeStates[nodeId] = state;
      }

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

    let disposed = false;
    let pendingFlush = false;
    const onAtomChange = () => {
      if (!topoEffectRanRef.current) return;
      if (pendingFlush) return;
      pendingFlush = true;
      queueMicrotask(() => {
        pendingFlush = false;
        if (disposed) return;
        applyPatch();
      });
    };

    const unsubs = nodeIds.map((id) =>
      sessionStore.sub(nodeStateAtom(nodeKey(selectedSessionId, id)), onAtomChange)
    );

    return () => {
      disposed = true;
      unsubs.forEach((u) => u());
    };
  }, [selectedSessionId, setEdges, pipelineRef, topoKey]);

  return { topoEffectRanRef };
}
