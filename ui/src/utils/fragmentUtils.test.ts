// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { Edge, Node } from '@xyflow/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { InputPin, NodeDefinition, OutputPin } from '@/types/generated/api-types';

import {
  extractFragment,
  fragmentToReactFlow,
  generateNodeId,
  type FragmentData,
} from './fragmentUtils';

// fragmentUtils.ts keeps a module-scoped counter for generateNodeId. Tests
// here must NOT assert on specific id strings (e.g. `skitnode_1`) — they may
// run in any order. Use the returned `idMapping` to resolve labels to ids,
// or assert relative ordering as in the generateNodeId block.

function idFor(idMapping: Map<string, string>, label: string): string {
  const id = idMapping.get(label);
  if (!id) throw new Error(`missing id for label "${label}"`);
  return id;
}

const { warnMock } = vi.hoisted(() => ({ warnMock: vi.fn() }));
vi.mock('@/utils/logger', () => ({
  utilsLogger: {
    warn: warnMock,
    info: vi.fn(),
    debug: vi.fn(),
    error: vi.fn(),
  },
}));

function makeDef(
  kind: string,
  options: {
    inputs?: InputPin[];
    outputs?: OutputPin[];
    bidirectional?: boolean;
    paramSchema?: unknown;
  } = {}
): NodeDefinition {
  return {
    kind,
    description: null,
    param_schema: options.paramSchema ?? {},
    inputs: options.inputs ?? [],
    outputs: options.outputs ?? [],
    categories: [],
    bidirectional: options.bidirectional ?? false,
  };
}

const HANDLERS = {
  onParamChange: vi.fn(),
  onLabelChange: vi.fn(),
};

beforeEach(() => {
  warnMock.mockReset();
  HANDLERS.onParamChange.mockReset();
  HANDLERS.onLabelChange.mockReset();
});

describe('generateNodeId', () => {
  it('produces sequential, prefixed, unique ids', () => {
    const a = generateNodeId();
    const b = generateNodeId();
    const c = generateNodeId();
    expect(a).toMatch(/^skitnode_\d+$/);
    expect(b).toMatch(/^skitnode_\d+$/);
    expect(c).toMatch(/^skitnode_\d+$/);
    expect(new Set([a, b, c]).size).toBe(3);

    const idNum = (id: string) => Number(id.replace('skitnode_', ''));
    expect(idNum(b)).toBe(idNum(a) + 1);
    expect(idNum(c)).toBe(idNum(b) + 1);
  });
});

describe('fragmentToReactFlow', () => {
  const FRAGMENT: FragmentData = {
    nodes: {
      src: { kind: 'audio::source' },
      gain: { kind: 'audio::gain', params: { db: -3 }, needs: 'src' },
      sink: { kind: 'audio::sink', needs: ['gain'] },
    },
  };

  const DEFS: NodeDefinition[] = [
    makeDef('audio::source', {
      outputs: [{ name: 'out', produces_type: 'Any', cardinality: 'One' }],
    }),
    makeDef('audio::gain', {
      inputs: [{ name: 'in', accepts_types: ['Any'], cardinality: 'One' }],
      outputs: [{ name: 'out', produces_type: 'Any', cardinality: 'One' }],
      paramSchema: { properties: { db: { type: 'number' } } },
    }),
    makeDef('audio::sink', {
      inputs: [{ name: 'in', accepts_types: ['Any'], cardinality: 'One' }],
    }),
  ];

  function buildSampleFragment() {
    const labelCounts = new Map<string, number>();
    const nextLabelForKind = (kind: string) => {
      const next = (labelCounts.get(kind) ?? 0) + 1;
      labelCounts.set(kind, next);
      return `${kind}_${next}`;
    };

    return fragmentToReactFlow(FRAGMENT, { x: 50, y: 100 }, DEFS, HANDLERS, nextLabelForKind);
  }

  it('returns one node and edge per fragment entry/dep with a populated id mapping', () => {
    const { nodes, edges, idMapping } = buildSampleFragment();
    expect(nodes).toHaveLength(3);
    expect(edges).toHaveLength(2);
    expect(idMapping.size).toBe(3);
  });

  it('selects "audioGain" node type for audio::gain and "configurable" for everything else', () => {
    const { nodes, idMapping } = buildSampleFragment();
    expect(nodes.find((n) => n.id === idFor(idMapping, 'src'))!.type).toBe('configurable');
    expect(nodes.find((n) => n.id === idFor(idMapping, 'gain'))!.type).toBe('audioGain');
    expect(nodes.find((n) => n.id === idFor(idMapping, 'sink'))!.type).toBe('configurable');
  });

  it('places nodes into a grid offset from the given anchor position', () => {
    const { nodes, idMapping } = buildSampleFragment();
    // 3 nodes → ceil(sqrt(3)) = 2 columns. Indices 0,1,2 → (col,row): (0,0),(1,0),(0,1).
    expect(nodes.find((n) => n.id === idFor(idMapping, 'src'))!.position).toEqual({
      x: 50,
      y: 100,
    });
    expect(nodes.find((n) => n.id === idFor(idMapping, 'gain'))!.position).toEqual({
      x: 250,
      y: 100,
    });
    expect(nodes.find((n) => n.id === idFor(idMapping, 'sink'))!.position).toEqual({
      x: 50,
      y: 250,
    });
  });

  it('populates data fields from the matched NodeDefinition and supplied handlers', () => {
    const { nodes, idMapping } = buildSampleFragment();
    const gainNode = nodes.find((n) => n.id === idFor(idMapping, 'gain'))!;
    expect(gainNode.dragHandle).toBe('.drag-handle');
    expect(gainNode.selected).toBe(false);
    expect(gainNode.data.params).toEqual({ db: -3 });
    expect(gainNode.data.kind).toBe('audio::gain');
    expect(gainNode.data.label).toBe('audio::gain_1');
    expect(gainNode.data.inputs).toEqual(DEFS[1].inputs);
    expect(gainNode.data.outputs).toEqual(DEFS[1].outputs);
    expect((gainNode.data.definition as { bidirectional: boolean }).bidirectional).toBe(false);
    expect(gainNode.data.onParamChange).toBe(HANDLERS.onParamChange);
    expect(gainNode.data.onLabelChange).toBe(HANDLERS.onLabelChange);
  });

  it('defaults missing params to an empty object', () => {
    const { nodes, idMapping } = buildSampleFragment();
    const srcNode = nodes.find((n) => n.id === idFor(idMapping, 'src'))!;
    expect(srcNode.data.params).toEqual({});
  });

  it('stitches dependency edges with a deterministic id and the in/out handle names', () => {
    const { edges, idMapping } = buildSampleFragment();
    const srcId = idFor(idMapping, 'src');
    const gainId = idFor(idMapping, 'gain');
    const sinkId = idFor(idMapping, 'sink');

    const srcGain = edges.find((e) => e.source === srcId && e.target === gainId)!;
    expect(srcGain.id).toBe(`${srcId}_out_${gainId}_in`);
    expect(srcGain.sourceHandle).toBe('out');
    expect(srcGain.targetHandle).toBe('in');
    expect(srcGain.type).toBe('default');

    expect(edges.find((e) => e.source === gainId && e.target === sinkId)).toBeDefined();
  });

  it('falls back to empty inputs/outputs when the node kind is not in the definitions list', () => {
    const fragment: FragmentData = {
      nodes: {
        unknown: { kind: 'not::registered' },
      },
    };
    const { nodes, edges } = fragmentToReactFlow(
      fragment,
      { x: 0, y: 0 },
      [],
      HANDLERS,
      (k) => `${k}_label`
    );
    expect(nodes).toHaveLength(1);
    expect(edges).toEqual([]);
    expect(nodes[0].data.inputs).toEqual([]);
    expect(nodes[0].data.outputs).toEqual([]);
    expect(nodes[0].data.nodeDefinition).toBeUndefined();
  });

  it('warns and skips edges when a needs label references a node not in the fragment', () => {
    const fragment: FragmentData = {
      nodes: {
        orphan: { kind: 'audio::sink', needs: 'missing-source' },
      },
    };
    const { edges } = fragmentToReactFlow(
      fragment,
      { x: 0, y: 0 },
      [makeDef('audio::sink')],
      HANDLERS,
      (k) => `${k}_label`
    );
    expect(edges).toEqual([]);
    expect(warnMock).toHaveBeenCalledTimes(1);
    expect(warnMock.mock.calls[0][0]).toMatch(/Could not find node mapping for dependency/);
  });

  it('returns an empty result for a fragment with no nodes', () => {
    const { nodes, edges, idMapping } = fragmentToReactFlow(
      { nodes: {} },
      { x: 0, y: 0 },
      [],
      HANDLERS,
      (k) => k
    );
    expect(nodes).toEqual([]);
    expect(edges).toEqual([]);
    expect(idMapping.size).toBe(0);
  });
});

describe('extractFragment', () => {
  it('selects only nodes in the selection set and stitches their internal needs by label', () => {
    const nodes: Node[] = [
      {
        id: 'n1',
        position: { x: 0, y: 0 },
        data: { kind: 'audio::source', label: 'src', params: {} },
      },
      {
        id: 'n2',
        position: { x: 0, y: 0 },
        data: { kind: 'audio::gain', label: 'gain', params: { db: -6 } },
      },
      {
        id: 'n3',
        position: { x: 0, y: 0 },
        data: { kind: 'audio::sink', label: 'sink', params: {} },
      },
    ] as Node[];

    const edges: Edge[] = [
      { id: 'e1', source: 'n1', target: 'n2' },
      { id: 'e2', source: 'n2', target: 'n3' },
    ];

    const fragment = extractFragment(['n1', 'n2'], nodes, edges);

    expect(Object.keys(fragment.nodes).sort()).toEqual(['gain', 'src']);
    expect(fragment.nodes.src).toEqual({ kind: 'audio::source', params: {} });
    expect(fragment.nodes.gain).toEqual({
      kind: 'audio::gain',
      params: { db: -6 },
      needs: 'src',
    });
  });

  it('uses an array for needs when a target has multiple selected dependencies', () => {
    const nodes: Node[] = [
      { id: 'a', position: { x: 0, y: 0 }, data: { kind: 'k', label: 'a', params: {} } },
      { id: 'b', position: { x: 0, y: 0 }, data: { kind: 'k', label: 'b', params: {} } },
      { id: 'c', position: { x: 0, y: 0 }, data: { kind: 'k', label: 'c', params: {} } },
    ] as Node[];

    const edges: Edge[] = [
      { id: 'e1', source: 'a', target: 'c' },
      { id: 'e2', source: 'b', target: 'c' },
    ];

    const fragment = extractFragment(['a', 'b', 'c'], nodes, edges);
    expect(fragment.nodes.c.needs).toEqual(['a', 'b']);
  });

  it('omits needs entirely when no incoming edges are selected', () => {
    const nodes: Node[] = [
      { id: 'a', position: { x: 0, y: 0 }, data: { kind: 'k', label: 'a', params: {} } },
      { id: 'b', position: { x: 0, y: 0 }, data: { kind: 'k', label: 'b', params: {} } },
    ] as Node[];
    const edges: Edge[] = [{ id: 'e1', source: 'a', target: 'b' }];

    // Only b is selected; the edge from a (not selected) → b is dropped.
    const fragment = extractFragment(['b'], nodes, edges);
    expect(fragment.nodes.b).toEqual({ kind: 'k', params: {} });
    expect(fragment.nodes.b.needs).toBeUndefined();
  });

  it('falls back to the node id when a node has no label, and to "unknown" when there is no kind', () => {
    const nodes: Node[] = [{ id: 'a', position: { x: 0, y: 0 }, data: {} }] as unknown as Node[];
    const fragment = extractFragment(['a'], nodes, []);
    expect(fragment.nodes.a).toEqual({ kind: 'unknown', params: {} });
  });

  it('returns an empty fragment when nothing is selected', () => {
    const nodes: Node[] = [
      { id: 'a', position: { x: 0, y: 0 }, data: { kind: 'k', label: 'a' } },
    ] as Node[];
    expect(extractFragment([], nodes, [])).toEqual({ nodes: {} });
  });

  it('round-trips through fragmentToReactFlow: extracted needs resolve to the new ids', () => {
    const originalNodes: Node[] = [
      {
        id: 'orig-1',
        position: { x: 0, y: 0 },
        data: { kind: 'audio::source', label: 'src', params: {} },
      },
      {
        id: 'orig-2',
        position: { x: 0, y: 0 },
        data: { kind: 'audio::sink', label: 'sink', params: {} },
      },
    ] as Node[];
    const originalEdges: Edge[] = [{ id: 'e1', source: 'orig-1', target: 'orig-2' }];

    const fragment = extractFragment(['orig-1', 'orig-2'], originalNodes, originalEdges);

    const defs: NodeDefinition[] = [makeDef('audio::source'), makeDef('audio::sink')];
    const { nodes, edges, idMapping } = fragmentToReactFlow(
      fragment,
      { x: 0, y: 0 },
      defs,
      HANDLERS,
      (k) => `${k}_label`
    );

    expect(nodes).toHaveLength(2);
    expect(edges).toHaveLength(1);
    expect(edges[0].source).toBe(idMapping.get('src'));
    expect(edges[0].target).toBe(idMapping.get('sink'));
  });
});
