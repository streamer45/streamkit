// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { Pipeline } from '@/types/types';

export type SimpleEdge = { source: string; target: string };

/** Returns true if adding `newEdge` would create a cycle (bidirectional nodes exempt). */
export function wouldCreateCycle(
  nodeIds: string[],
  existingEdges: SimpleEdge[],
  newEdge: SimpleEdge,
  bidirectionalNodeIds: string[] = []
): boolean {
  const allEdges = [...existingEdges, newEdge];

  const inDegree: Record<string, number> = {};
  const outgoing: Record<string, string[]> = {};

  nodeIds.forEach((id) => {
    inDegree[id] = 0;
    outgoing[id] = [];
  });

  for (const e of allEdges) {
    if (!(e.source in outgoing) || !(e.target in inDegree)) continue;
    outgoing[e.source].push(e.target);
    inDegree[e.target] += 1;
  }

  const queue: string[] = [];
  for (const id of nodeIds) {
    if (inDegree[id] === 0) queue.push(id);
  }

  let processedCount = 0;
  const processed = new Set<string>();
  while (queue.length > 0) {
    const u = queue.shift() as string;
    processedCount++;
    processed.add(u);

    for (const v of outgoing[u]) {
      inDegree[v] -= 1;
      if (inDegree[v] === 0) {
        queue.push(v);
      }
    }
  }

  if (processedCount === nodeIds.length) {
    return false; // No cycle
  }

  const nodesInCycle = nodeIds.filter((id) => !processed.has(id));
  const hasBidirectionalNode = nodesInCycle.some((id) => bidirectionalNodeIds.includes(id));

  return !hasBidirectionalNode;
}

function initializeGraphStructures(
  nodeIds: string[],
  edges: SimpleEdge[]
): {
  inDegree: Record<string, number>;
  outgoing: Record<string, string[]>;
  predecessors: Record<string, string[]>;
} {
  const inDegree: Record<string, number> = {};
  const outgoing: Record<string, string[]> = {};
  const predecessors: Record<string, string[]> = {};

  for (const n of nodeIds) {
    inDegree[n] = 0;
    outgoing[n] = [];
    predecessors[n] = [];
  }

  for (const e of edges) {
    if (!(e.source in outgoing) || !(e.target in inDegree)) continue;
    outgoing[e.source].push(e.target);
    predecessors[e.target].push(e.source);
    inDegree[e.target] += 1;
  }

  return { inDegree, outgoing, predecessors };
}

function computeTopologicalLevels(
  nodeIds: string[],
  inDegree: Record<string, number>,
  outgoing: Record<string, string[]>
): Record<string, number> {
  const level: Record<string, number> = {};
  const queue: string[] = [];

  for (const n of nodeIds) {
    if (inDegree[n] === 0) {
      queue.push(n);
      level[n] = 0;
    }
  }

  while (queue.length > 0) {
    const u = queue.shift() as string;
    for (const v of outgoing[u]) {
      level[v] = Math.max(level[v] ?? 0, (level[u] ?? 0) + 1);
      inDegree[v] -= 1;
      if (inDegree[v] === 0) {
        queue.push(v);
      }
    }
  }

  return level;
}

function assignRemainingLevels(
  nodeIds: string[],
  level: Record<string, number>,
  predecessors: Record<string, string[]>
): void {
  const assigned = new Set(Object.keys(level));
  if (assigned.size === nodeIds.length) {
    return; // All nodes already assigned
  }

  const maxAssignedLevel = assigned.size > 0 ? Math.max(...Object.values(level)) : 0;

  for (const n of nodeIds) {
    if (!assigned.has(n)) {
      const predLvls = (predecessors[n] || []).map((p) => (level[p] ?? maxAssignedLevel) + 1);
      level[n] = predLvls.length ? Math.max(...predLvls) : maxAssignedLevel + 1;
    }
  }
}

function groupNodesByLevel(
  nodeIds: string[],
  level: Record<string, number>
): { levels: Record<number, string[]>; sortedLevels: number[] } {
  const levels: Record<number, string[]> = {};
  for (const n of nodeIds) {
    const l = level[n] ?? 0;
    if (!levels[l]) levels[l] = [];
    levels[l].push(n);
  }

  const sortedLevels = Object.keys(levels)
    .map(Number)
    .sort((a, b) => a - b);

  return { levels, sortedLevels };
}

export function topoLevelsFromEdges(
  nodeIds: string[],
  edges: SimpleEdge[]
): {
  levels: Record<number, string[]>;
  sortedLevels: number[];
  levelByNode: Record<string, number>;
} {
  const { inDegree, outgoing, predecessors } = initializeGraphStructures(nodeIds, edges);

  const level = computeTopologicalLevels(nodeIds, inDegree, outgoing);

  assignRemainingLevels(nodeIds, level, predecessors);

  const { levels, sortedLevels } = groupNodesByLevel(nodeIds, level);

  return { levels, sortedLevels, levelByNode: level };
}

export function topoLevelsFromPipeline(pipeline: Pipeline): {
  levels: Record<number, string[]>;
  sortedLevels: number[];
  levelByNode: Record<string, number>;
} {
  const nodeIds = Object.keys(pipeline.nodes);
  const edges = pipeline.connections.map((conn) => ({
    source: conn.from_node,
    target: conn.to_node,
  }));
  return topoLevelsFromEdges(nodeIds, edges);
}

export function orderedNamesFromLevels(
  levels: Record<number, string[]>,
  sortedLevels: number[]
): string[] {
  const ordered: string[] = [];
  for (const l of sortedLevels) {
    for (const n of levels[l]) {
      ordered.push(n);
    }
  }
  return ordered;
}

type Adjacency = {
  outgoing: Record<string, string[]>;
  incoming: Record<string, string[]>;
};

function buildAdjacency(nodeIds: string[], edges: SimpleEdge[]): Adjacency {
  const outgoing: Record<string, string[]> = {};
  const incoming: Record<string, string[]> = {};

  for (const n of nodeIds) {
    outgoing[n] = [];
    incoming[n] = [];
  }

  for (const e of edges) {
    if (e.source in outgoing && e.target in incoming) {
      outgoing[e.source].push(e.target);
      incoming[e.target].push(e.source);
    }
  }

  return { outgoing, incoming };
}

function computeDepthByNode(
  sortedLevels: number[],
  levels: Record<number, string[]>,
  outgoing: Record<string, string[]>
): Record<string, number> {
  const depthByNode: Record<string, number> = {};

  for (const level of [...sortedLevels].reverse()) {
    for (const node of levels[level] ?? []) {
      const children = outgoing[node] ?? [];
      let bestChildDepth = 0;
      for (const child of children) {
        bestChildDepth = Math.max(bestChildDepth, depthByNode[child] ?? 0);
      }
      depthByNode[node] = children.length ? bestChildDepth + 1 : 0;
    }
  }

  return depthByNode;
}

function pickPrimaryChild(
  children: string[],
  outgoing: Record<string, string[]>,
  depthByNode: Record<string, number>
): string | undefined {
  if (children.length === 0) return undefined;
  if (children.length === 1) return children[0];

  let bestChild = children[0] as string;
  for (const child of children.slice(1)) {
    const childDepth = depthByNode[child] ?? 0;
    const bestDepth = depthByNode[bestChild] ?? 0;
    if (childDepth > bestDepth) {
      bestChild = child;
      continue;
    }
    if (childDepth < bestDepth) continue;

    const childOutDegree = (outgoing[child] ?? []).length;
    const bestOutDegree = (outgoing[bestChild] ?? []).length;
    if (childOutDegree > bestOutDegree) {
      bestChild = child;
    }
  }

  return bestChild;
}

function computePrimaryChildByParent(
  nodeIds: string[],
  outgoing: Record<string, string[]>,
  depthByNode: Record<string, number>
): Record<string, string> {
  const primaryChildByParent: Record<string, string> = {};

  for (const parent of nodeIds) {
    const children = outgoing[parent] ?? [];
    const primary = pickPrimaryChild(children, outgoing, depthByNode);
    if (primary && children.length > 1) {
      primaryChildByParent[parent] = primary;
    }
  }

  return primaryChildByParent;
}

function computeJoinLane(parents: string[], lanes: Record<string, number>): number | undefined {
  const parentLanes = parents.map((p) => lanes[p]).filter((l) => l !== undefined);
  if (parentLanes.length === 0) return undefined;
  return parentLanes.reduce((a, b) => a + b, 0) / parentLanes.length;
}

function computeLaneForNode(
  node: string,
  parents: string[],
  outgoing: Record<string, string[]>,
  primaryChildByParent: Record<string, string>,
  lanes: Record<string, number>,
  nextLane: number
): { lane: number; nextLane: number } {
  if (parents.length === 0) {
    return { lane: nextLane, nextLane: nextLane + 1 };
  }

  if (parents.length === 1) {
    const parent = parents[0] as string;
    const parentLane = lanes[parent];
    const parentChildren = outgoing[parent] ?? [];

    if (parentChildren.length === 1 && parentLane !== undefined) {
      return { lane: parentLane, nextLane };
    }

    if (primaryChildByParent[parent] === node && parentLane !== undefined) {
      return { lane: parentLane, nextLane };
    }

    return { lane: nextLane, nextLane: nextLane + 1 };
  }

  const joinLane = computeJoinLane(parents, lanes);
  if (joinLane !== undefined) {
    return { lane: joinLane, nextLane };
  }

  return { lane: nextLane, nextLane: nextLane + 1 };
}

function assignLanes(
  nodeIds: string[],
  edges: SimpleEdge[],
  levels: Record<number, string[]>,
  sortedLevels: number[]
): Record<string, number> {
  const { outgoing, incoming } = buildAdjacency(nodeIds, edges);

  // Primary branch heuristic: keep the child with the greatest downstream depth in the same lane.
  const depthByNode = computeDepthByNode(sortedLevels, levels, outgoing);
  const primaryChildByParent = computePrimaryChildByParent(nodeIds, outgoing, depthByNode);

  const lanes: Record<string, number> = {};
  let nextLane = 0;

  for (const level of sortedLevels) {
    const nodesAtLevel = levels[level] ?? [];

    for (const node of nodesAtLevel) {
      const parents = incoming[node] ?? [];
      const result = computeLaneForNode(
        node,
        parents,
        outgoing,
        primaryChildByParent,
        lanes,
        nextLane
      );
      lanes[node] = result.lane;
      nextLane = result.nextLane;
    }
  }

  return lanes;
}

function applyLaneBasedLayout(
  levels: Record<number, string[]>,
  sortedLevels: number[],
  lanes: Record<string, number>,
  nodeWidth: number,
  nodeHeight: number,
  hGap: number,
  vGap: number,
  heights: Record<string, number>,
  edges: SimpleEdge[]
): Record<string, { x: number; y: number }> {
  const positions: Record<string, { x: number; y: number }> = {};
  let yOffset = 0;

  const spacing = nodeWidth + hGap;

  const incoming: Record<string, string[]> = {};
  const nodeIds = sortedLevels.flatMap((l) => levels[l]);
  for (const n of nodeIds) {
    incoming[n] = [];
  }
  for (const e of edges) {
    if (e.target in incoming) {
      incoming[e.target].push(e.source);
    }
  }

  const median = (xs: number[]): number => {
    if (xs.length === 0) return 0;
    const sorted = [...xs].sort((a, b) => a - b);
    const mid = Math.floor(sorted.length / 2);
    if (sorted.length % 2 === 1) return sorted[mid] as number;
    const a = sorted[mid - 1] as number;
    const b = sorted[mid] as number;
    return (a + b) / 2;
  };

  for (const l of sortedLevels) {
    const names = levels[l];
    const levelMaxH = names.length
      ? Math.max(...names.map((n) => heights[n] ?? nodeHeight))
      : nodeHeight;

    const ordered = [...names].sort((a, b) => {
      const la = lanes[a] ?? 0;
      const lb = lanes[b] ?? 0;
      if (la !== lb) return la - lb;
      return a.localeCompare(b);
    });

    const deltas = ordered.map((name, idx) => {
      const lane = lanes[name] ?? 0;
      return lane * spacing - idx * spacing;
    });

    const continuityIdx = ordered.findIndex((name) => {
      const parents = incoming[name] ?? [];
      if (parents.length !== 1) return false;
      const parent = parents[0] as string;
      const ln = lanes[name];
      const lp = lanes[parent];
      if (ln === undefined || lp === undefined) return false;
      return Math.abs(ln - lp) < 1e-9;
    });

    const rowOffset = continuityIdx >= 0 ? (deltas[continuityIdx] as number) : median(deltas);

    ordered.forEach((name, idx) => {
      const x = idx * spacing + rowOffset;
      const y = yOffset;
      positions[name] = { x, y };
    });

    yOffset += levelMaxH + vGap;
  }

  return positions;
}

function applyCenteredLayout(
  levels: Record<number, string[]>,
  sortedLevels: number[],
  nodeWidth: number,
  nodeHeight: number,
  hGap: number,
  vGap: number,
  heights: Record<string, number>
): Record<string, { x: number; y: number }> {
  const positions: Record<string, { x: number; y: number }> = {};
  const maxCols = sortedLevels.reduce((m, l) => Math.max(m, levels[l].length), 0);
  let yOffset = 0;

  for (const l of sortedLevels) {
    const names = levels[l];
    const rowCount = names.length;
    const startColumn = Math.floor((maxCols - rowCount) / 2);

    const levelMaxH = names.length
      ? Math.max(...names.map((n) => heights[n] ?? nodeHeight))
      : nodeHeight;

    names.forEach((name, idx) => {
      const col = startColumn + idx;
      const x = col * (nodeWidth + hGap);
      const y = yOffset;
      positions[name] = { x, y };
    });

    yOffset += levelMaxH + vGap;
  }

  return positions;
}

function computeLanesIfNeeded(
  edges: SimpleEdge[],
  nodeIds: string[],
  levels: Record<number, string[]>,
  sortedLevels: number[]
): Record<string, number> {
  if (edges.length === 0) {
    return {};
  }
  return assignLanes(nodeIds, edges, levels, sortedLevels);
}

export function verticalLayout(
  levels: Record<number, string[]>,
  sortedLevels: number[],
  opts?: {
    nodeWidth?: number;
    nodeHeight?: number; // fallback height if a node's actual/estimated height is unknown
    hGap?: number;
    vGap?: number; // desired vertical gap between levels (space between nodes)
    heights?: Record<string, number>; // optional per-node height map to keep edge spacing consistent
    edges?: SimpleEdge[]; // edges for lane assignment
  }
): Record<string, { x: number; y: number }> {
  const nodeWidth = opts?.nodeWidth ?? 250;
  const nodeHeight = opts?.nodeHeight ?? 150;
  const hGap = opts?.hGap ?? 80;
  const vGap = opts?.vGap ?? 60;
  const heights = opts?.heights ?? {};
  const edges = opts?.edges ?? [];

  const nodeIds = sortedLevels.flatMap((l) => levels[l]);
  const lanes = computeLanesIfNeeded(edges, nodeIds, levels, sortedLevels);

  const hasValidLanes = Object.keys(lanes).length > 0;
  if (hasValidLanes) {
    return applyLaneBasedLayout(
      levels,
      sortedLevels,
      lanes,
      nodeWidth,
      nodeHeight,
      hGap,
      vGap,
      heights,
      edges
    );
  }

  return applyCenteredLayout(levels, sortedLevels, nodeWidth, nodeHeight, hGap, vGap, heights);
}
