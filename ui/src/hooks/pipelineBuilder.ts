// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Pure functions for building the pipeline YAML representation from
 * the canvas graph.  Extracted from usePipeline to keep the hook
 * within the file-length lint limit.
 */

import type { Node, Edge } from '@xyflow/react';

import { sessionStore as defaultSessionStore, nodeParamsAtom } from '@/stores/sessionAtoms';
import type { EngineMode } from '@/utils/yamlPipeline';

type EditorNodeData = {
  label: string;
  kind: string;
  params?: Record<string, unknown>;
  ui?: { position?: { x: number; y: number } };
  paramSchema?: unknown;
  inputs?: unknown;
  outputs?: unknown;
  definition?: { bidirectional?: boolean };
  onParamChange?: (nodeId: string, paramName: string, value: unknown) => void;
  onLabelChange?: (nodeId: string, newLabel: string) => void;
};

type ConnectionMode = 'reliable' | 'best_effort';
type NeedsDependency = string | { node: string; mode?: ConnectionMode };

export type { EditorNodeData, ConnectionMode };

export function orderNodeIdsTopDown(
  nodes: Array<Node<EditorNodeData>>,
  edges: Array<Edge>
): Array<string> {
  const nodeIds = nodes.map((n) => n.id);
  const posById = new Map(nodeIds.map((id) => [id, { x: 0, y: 0 }]));
  nodes.forEach((n) => posById.set(n.id, { x: n.position.x, y: n.position.y }));

  const inDegree: Record<string, number> = {};
  const outgoing: Record<string, string[]> = {};
  nodeIds.forEach((nodeId) => {
    inDegree[nodeId] = 0;
    outgoing[nodeId] = [];
  });

  edges.forEach((e) => {
    if (!(e.source in outgoing) || !(e.target in inDegree)) return;
    outgoing[e.source].push(e.target);
    inDegree[e.target] += 1;
  });

  const compare = (a: string, b: string) => {
    const pa = posById.get(a) ?? { x: 0, y: 0 };
    const pb = posById.get(b) ?? { x: 0, y: 0 };
    if (pa.y !== pb.y) return pa.y - pb.y;
    if (pa.x !== pb.x) return pa.x - pb.x;
    return a.localeCompare(b);
  };

  const queue = nodeIds.filter((nodeId) => inDegree[nodeId] === 0).sort(compare);
  const ordered: string[] = [];
  const seen = new Set<string>();

  while (queue.length > 0) {
    const u = queue.shift() as string;
    if (seen.has(u)) continue;
    seen.add(u);
    ordered.push(u);
    for (const v of outgoing[u]) {
      inDegree[v] -= 1;
      if (inDegree[v] === 0) {
        queue.push(v);
      }
    }
    queue.sort(compare);
  }

  const remaining = nodeIds.filter((nodeId) => !seen.has(nodeId)).sort(compare);
  return [...ordered, ...remaining];
}

export function buildPipelineForYaml(
  nodes: Array<Node<EditorNodeData>>,
  edges: Array<Edge>,
  mode: EngineMode,
  opts?: { includeUiPositions?: boolean }
): { mode: EngineMode; nodes: Record<string, unknown> } {
  const includeUiPositions = opts?.includeUiPositions ?? false;
  const idToLabelMap = new Map(nodes.map((n) => [n.id, n.data.label]));
  const idToNode = new Map(nodes.map((n) => [n.id, n]));
  const pipeline: { mode: EngineMode; nodes: Record<string, unknown> } = { mode, nodes: {} };

  const orderedIds = orderNodeIdsTopDown(nodes, edges);

  orderedIds.forEach((nodeId) => {
    const node = idToNode.get(nodeId);
    if (!node) return;

    const needs = edges
      .filter((e) => e.target === node.id)
      .map((e): NeedsDependency | null => {
        const label = idToLabelMap.get(e.source);
        if (!label) return null;
        const sourceNode = idToNode.get(e.source);
        const sourceOutputs = (sourceNode?.data.outputs || []) as Array<{ name: string }>;
        const defaultOutput = sourceOutputs[0]?.name;
        const sourceHandle = e.sourceHandle || defaultOutput;
        const annotatePin =
          sourceOutputs.length > 1 ||
          (sourceHandle && defaultOutput && sourceHandle !== defaultOutput);

        const needsLabel = sourceHandle && annotatePin ? `${label}.${sourceHandle}` : label;
        const connMode = (e.data as { mode?: ConnectionMode } | undefined)?.mode;
        return connMode === 'best_effort' ? { node: needsLabel, mode: connMode } : needsLabel;
      })
      .filter((v): v is NeedsDependency => v !== null);

    const nodeConfig: Record<string, unknown> = { kind: node.data.kind };

    if (includeUiPositions) {
      nodeConfig['ui'] = {
        position: {
          x: Math.round(node.position.x),
          y: Math.round(node.position.y),
        },
      };
    }

    const overrides = defaultSessionStore.get(nodeParamsAtom(node.id));
    const mergedParams = { ...(node.data.params || {}), ...(overrides || {}) };
    if (Object.keys(mergedParams).length > 0) {
      nodeConfig['params'] = mergedParams;
    }

    if (needs.length === 1) {
      nodeConfig['needs'] = needs[0];
    } else if (needs.length > 1) {
      nodeConfig['needs'] = needs;
    }

    pipeline.nodes[node.data.label] = nodeConfig;
  });

  return pipeline;
}
