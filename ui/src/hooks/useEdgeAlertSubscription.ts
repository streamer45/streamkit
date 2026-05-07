// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

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
  nodeStates: Map<string, NodeState | null>
): {
  slowPinsByNode: Map<string, Set<string>>;
  slowDetailsByNode: Map<string, SlowTimeoutDetails>;
} {
  const slowPinsByNode = new Map<string, Set<string>>();
  const slowDetailsByNode = new Map<string, SlowTimeoutDetails>();
  for (const [nodeId, apiNode] of Object.entries(pipeline.nodes)) {
    const st = nodeStates.get(nodeId) ?? apiNode.state ?? null;
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

function readNodeStates(sessionId: string, pipeline: Pipeline): Map<string, NodeState | null> {
  const states = new Map<string, NodeState | null>();
  for (const id of Object.keys(pipeline.nodes)) {
    states.set(id, sessionStore.get(nodeStateAtom(nodeKey(sessionId, id))));
  }
  return states;
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

    // Reset so patches are gated until the topology effect rebuilds the graph.
    // The topology effect (which runs after this one) sets it back to true.
    topoEffectRanRef.current = false;

    const applyPatch = () => {
      const currentPipeline = pipelineRef.current;
      if (!currentPipeline) return;

      const nodeStates = readNodeStates(selectedSessionId, currentPipeline);

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

    const pipeline = pipelineRef.current;
    if (!pipeline) return;

    const unsubs = Object.keys(pipeline.nodes).map((id) => {
      const atom = nodeStateAtom(nodeKey(selectedSessionId, id));
      return sessionStore.sub(atom, () => {
        if (!topoEffectRanRef.current) return;
        applyPatch();
      });
    });

    return () => {
      for (const unsub of unsubs) unsub();
    };
  }, [selectedSessionId, setEdges, pipelineRef, topoKey]);

  return { topoEffectRanRef };
}
