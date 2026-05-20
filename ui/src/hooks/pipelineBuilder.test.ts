// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Tests for pipelineBuilder.  Uses the default sessionStore (the only one
// buildPipelineForYaml reads from) and cleans up per-test param overrides
// in afterEach via the tracked-ID set below.

import type { Edge, Node } from '@xyflow/react';
import { afterEach, describe, expect, it } from 'vitest';

import { nodeParamsAtom, sessionStore } from '@/stores/sessionAtoms';

import { buildPipelineForYaml, orderNodeIdsTopDown } from './pipelineBuilder';
import type { EditorNodeData } from './pipelineBuilder';

type EditorNode = Node<EditorNodeData>;

function makeNode(
  id: string,
  pos: { x: number; y: number },
  data: Partial<EditorNodeData> = {}
): EditorNode {
  // Cast skips React Flow runtime-only fields (measured, width, height,
  // selected, dragging, parentId, etc.) that orderNodeIdsTopDown and
  // buildPipelineForYaml don't read.  If the production code ever begins
  // depending on those, this cast will hide it — drop the cast then.
  return {
    id,
    position: pos,
    data: {
      label: data.label ?? id,
      kind: data.kind ?? 'audio::passthrough',
      params: data.params,
      outputs: data.outputs,
      ...data,
    },
    type: 'editor',
  } as EditorNode;
}

function makeEdge(
  id: string,
  source: string,
  target: string,
  opts: { sourceHandle?: string; mode?: 'reliable' | 'best_effort' } = {}
): Edge {
  return {
    id,
    source,
    target,
    sourceHandle: opts.sourceHandle,
    data: opts.mode ? { mode: opts.mode } : undefined,
  };
}

const trackedNodeIds: string[] = [];
function track(id: string): string {
  trackedNodeIds.push(id);
  return id;
}

afterEach(() => {
  // Reset every nodeParamsAtom touched by a test so cross-test pollution
  // can't carry param overrides between cases.  Removing the atomFamily
  // entry resets it to its default empty object.
  for (const id of trackedNodeIds) {
    nodeParamsAtom.remove(id);
  }
  trackedNodeIds.length = 0;
});

describe('orderNodeIdsTopDown', () => {
  it('returns the only node for a single-node graph', () => {
    const nodes = [makeNode('a', { x: 0, y: 0 })];
    expect(orderNodeIdsTopDown(nodes, [])).toEqual(['a']);
  });

  it('orders linear chains topologically', () => {
    const nodes = [
      makeNode('c', { x: 0, y: 200 }),
      makeNode('a', { x: 0, y: 0 }),
      makeNode('b', { x: 0, y: 100 }),
    ];
    const edges = [makeEdge('e0', 'a', 'b'), makeEdge('e1', 'b', 'c')];
    expect(orderNodeIdsTopDown(nodes, edges)).toEqual(['a', 'b', 'c']);
  });

  it('breaks ties by Y, then X, then ID', () => {
    const nodes = [
      makeNode('lo-id', { x: 50, y: 0 }),
      makeNode('hi-id', { x: 0, y: 0 }),
      makeNode('below', { x: 0, y: 100 }),
    ];
    const order = orderNodeIdsTopDown(nodes, []);
    // Same Y=0: x=0 (hi-id) wins over x=50 (lo-id); 'below' has y=100 so last.
    expect(order).toEqual(['hi-id', 'lo-id', 'below']);
  });

  it('falls back to ID comparison when both X and Y match', () => {
    const nodes = [makeNode('zzz', { x: 0, y: 0 }), makeNode('aaa', { x: 0, y: 0 })];
    expect(orderNodeIdsTopDown(nodes, [])).toEqual(['aaa', 'zzz']);
  });

  it('places source roots before downstream targets', () => {
    const nodes = [makeNode('target', { x: 0, y: 0 }), makeNode('source', { x: 0, y: 1000 })];
    const edges = [makeEdge('e0', 'source', 'target')];
    expect(orderNodeIdsTopDown(nodes, edges)).toEqual(['source', 'target']);
  });

  it('handles a diamond DAG with stable ordering', () => {
    const nodes = [
      makeNode('a', { x: 0, y: 0 }),
      makeNode('b', { x: 0, y: 100 }),
      makeNode('c', { x: 200, y: 100 }),
      makeNode('d', { x: 100, y: 200 }),
    ];
    const edges = [
      makeEdge('e0', 'a', 'b'),
      makeEdge('e1', 'a', 'c'),
      makeEdge('e2', 'b', 'd'),
      makeEdge('e3', 'c', 'd'),
    ];
    const order = orderNodeIdsTopDown(nodes, edges);
    expect(order[0]).toBe('a');
    expect(order[order.length - 1]).toBe('d');
    expect(order.indexOf('b')).toBeLessThan(order.indexOf('d'));
    expect(order.indexOf('c')).toBeLessThan(order.indexOf('d'));
  });

  it('ignores edges whose endpoints are not in the node list', () => {
    const nodes = [makeNode('a', { x: 0, y: 0 })];
    const edges = [makeEdge('e0', 'ghost', 'a'), makeEdge('e1', 'a', 'phantom')];
    expect(orderNodeIdsTopDown(nodes, edges)).toEqual(['a']);
  });

  it('terminates on a pure cycle and returns the cycle members sorted by Y', () => {
    // a → b → c → a forms a cycle with no in-degree-zero nodes.  The
    // function should terminate (no infinite loop) and emit the unseen
    // nodes sorted by the position-based comparator (Y → X → ID).
    const nodes = [
      makeNode('a', { x: 0, y: 0 }),
      makeNode('b', { x: 0, y: 100 }),
      makeNode('c', { x: 0, y: 200 }),
    ];
    const edges = [makeEdge('e0', 'a', 'b'), makeEdge('e1', 'b', 'c'), makeEdge('e2', 'c', 'a')];
    expect(orderNodeIdsTopDown(nodes, edges)).toEqual(['a', 'b', 'c']);
  });

  it('places DAG nodes before cycle members in a mixed graph', () => {
    // Independent DAG: root → leaf.  Separate cycle: x → y → z → x.  Even
    // though the cycle nodes are positioned above the DAG (lower Y), they
    // belong to the unseen-tail and must come after the resolved DAG.
    const nodes = [
      makeNode('root', { x: 0, y: 500 }),
      makeNode('leaf', { x: 0, y: 600 }),
      makeNode('x', { x: 0, y: 0 }),
      makeNode('y', { x: 0, y: 100 }),
      makeNode('z', { x: 0, y: 200 }),
    ];
    const edges = [
      makeEdge('e0', 'root', 'leaf'),
      makeEdge('e1', 'x', 'y'),
      makeEdge('e2', 'y', 'z'),
      makeEdge('e3', 'z', 'x'),
    ];
    const order = orderNodeIdsTopDown(nodes, edges);
    expect(order.indexOf('root')).toBeLessThan(order.indexOf('x'));
    expect(order.indexOf('leaf')).toBeLessThan(order.indexOf('x'));
    expect(order.slice(2)).toEqual(['x', 'y', 'z']);
  });
});

describe('buildPipelineForYaml — node kind / mode', () => {
  it('emits mode and per-node kind in topological order', () => {
    const nodes = [
      makeNode(track('a'), { x: 0, y: 0 }, { label: 'src', kind: 'audio::tone' }),
      makeNode(track('b'), { x: 0, y: 100 }, { label: 'sink', kind: 'audio::null_sink' }),
    ];
    const edges = [makeEdge('e0', 'a', 'b')];
    const yaml = buildPipelineForYaml(nodes, edges, 'oneshot');

    expect(yaml.mode).toBe('oneshot');
    expect(Object.keys(yaml.nodes)).toEqual(['src', 'sink']);
    expect((yaml.nodes['src'] as { kind: string }).kind).toBe('audio::tone');
  });

  it('honours dynamic mode', () => {
    const nodes = [makeNode(track('a'), { x: 0, y: 0 })];
    const yaml = buildPipelineForYaml(nodes, [], 'dynamic');
    expect(yaml.mode).toBe('dynamic');
  });
});

describe('buildPipelineForYaml — needs assembly', () => {
  it('omits needs when a node has no inbound edges', () => {
    const nodes = [makeNode(track('a'), { x: 0, y: 0 }, { label: 'src' })];
    const yaml = buildPipelineForYaml(nodes, [], 'oneshot');
    expect(yaml.nodes['src']).not.toHaveProperty('needs');
  });

  it('emits a string needs for a single dependency', () => {
    const nodes = [
      makeNode(track('a'), { x: 0, y: 0 }, { label: 'src' }),
      makeNode(track('b'), { x: 0, y: 100 }, { label: 'sink' }),
    ];
    const edges = [makeEdge('e0', 'a', 'b')];
    const yaml = buildPipelineForYaml(nodes, edges, 'oneshot');
    expect(yaml.nodes['sink']).toMatchObject({ needs: 'src' });
  });

  it('emits an array needs for multiple dependencies', () => {
    const nodes = [
      makeNode(track('a'), { x: 0, y: 0 }, { label: 'one' }),
      makeNode(track('b'), { x: 100, y: 0 }, { label: 'two' }),
      makeNode(track('c'), { x: 50, y: 100 }, { label: 'mix' }),
    ];
    const edges = [makeEdge('e0', 'a', 'c'), makeEdge('e1', 'b', 'c')];
    const yaml = buildPipelineForYaml(nodes, edges, 'oneshot');
    expect(yaml.nodes['mix']).toMatchObject({ needs: ['one', 'two'] });
  });

  it('annotates the source pin only when the source has >1 outputs', () => {
    const nodes = [
      makeNode(
        track('a'),
        { x: 0, y: 0 },
        {
          label: 'splitter',
          outputs: [{ name: 'left' }, { name: 'right' }],
        }
      ),
      makeNode(track('b'), { x: 0, y: 100 }, { label: 'consumer-l' }),
      makeNode(track('c'), { x: 100, y: 100 }, { label: 'consumer-r' }),
    ];
    const edges = [
      makeEdge('e0', 'a', 'b', { sourceHandle: 'left' }),
      makeEdge('e1', 'a', 'c', { sourceHandle: 'right' }),
    ];
    const yaml = buildPipelineForYaml(nodes, edges, 'oneshot');
    expect((yaml.nodes['consumer-l'] as { needs: string }).needs).toBe('splitter.left');
    expect((yaml.nodes['consumer-r'] as { needs: string }).needs).toBe('splitter.right');
  });

  it('does NOT annotate the pin when the source has a single output and the default is used', () => {
    const nodes = [
      makeNode(
        track('a'),
        { x: 0, y: 0 },
        {
          label: 'src',
          outputs: [{ name: 'out' }],
        }
      ),
      makeNode(track('b'), { x: 0, y: 100 }, { label: 'sink' }),
    ];
    const edges = [makeEdge('e0', 'a', 'b', { sourceHandle: 'out' })];
    const yaml = buildPipelineForYaml(nodes, edges, 'oneshot');
    expect((yaml.nodes['sink'] as { needs: string }).needs).toBe('src');
  });

  it('emits best_effort needs as an object with mode', () => {
    const nodes = [
      makeNode(track('a'), { x: 0, y: 0 }, { label: 'src' }),
      makeNode(track('b'), { x: 0, y: 100 }, { label: 'sink' }),
    ];
    const edges = [makeEdge('e0', 'a', 'b', { mode: 'best_effort' })];
    const yaml = buildPipelineForYaml(nodes, edges, 'oneshot');
    expect(yaml.nodes['sink']).toMatchObject({ needs: { node: 'src', mode: 'best_effort' } });
  });

  it('emits a plain string for reliable mode (the default)', () => {
    const nodes = [
      makeNode(track('a'), { x: 0, y: 0 }, { label: 'src' }),
      makeNode(track('b'), { x: 0, y: 100 }, { label: 'sink' }),
    ];
    const edges = [makeEdge('e0', 'a', 'b', { mode: 'reliable' })];
    const yaml = buildPipelineForYaml(nodes, edges, 'oneshot');
    expect(yaml.nodes['sink']).toMatchObject({ needs: 'src' });
  });

  it('skips edges whose source has no resolvable label', () => {
    const nodes = [makeNode(track('b'), { x: 0, y: 0 }, { label: 'sink' })];
    const edges = [makeEdge('e0', 'ghost', 'b')];
    const yaml = buildPipelineForYaml(nodes, edges, 'oneshot');
    expect(yaml.nodes['sink']).not.toHaveProperty('needs');
  });
});

describe('buildPipelineForYaml — params + ui blocks', () => {
  it('omits params when no params and no atom overrides exist', () => {
    const nodes = [makeNode(track('a'), { x: 0, y: 0 }, { label: 'n' })];
    const yaml = buildPipelineForYaml(nodes, [], 'oneshot');
    expect(yaml.nodes['n']).not.toHaveProperty('params');
  });

  it('includes node.data.params verbatim when no atom override', () => {
    const nodes = [
      makeNode(
        track('a'),
        { x: 0, y: 0 },
        {
          label: 'n',
          params: { volume: 0.7, codec: 'opus' },
        }
      ),
    ];
    const yaml = buildPipelineForYaml(nodes, [], 'oneshot');
    expect(yaml.nodes['n']).toMatchObject({
      params: { volume: 0.7, codec: 'opus' },
    });
  });

  it('layers atom overrides on top of node.data.params (override wins)', () => {
    const id = track('a-override');
    const nodes = [
      makeNode(
        id,
        { x: 0, y: 0 },
        {
          label: 'n',
          params: { volume: 0.5, mute: false },
        }
      ),
    ];
    sessionStore.set(nodeParamsAtom(id), { volume: 0.9, channel: 1 });

    const yaml = buildPipelineForYaml(nodes, [], 'oneshot');
    expect(yaml.nodes['n']).toMatchObject({
      params: { volume: 0.9, mute: false, channel: 1 },
    });
  });

  it('emits params from atom overrides alone when node.data.params is absent', () => {
    const id = track('a-only-override');
    const nodes = [makeNode(id, { x: 0, y: 0 }, { label: 'n' })];
    sessionStore.set(nodeParamsAtom(id), { gain_db: -6 });

    const yaml = buildPipelineForYaml(nodes, [], 'oneshot');
    expect(yaml.nodes['n']).toMatchObject({ params: { gain_db: -6 } });
  });

  it('omits ui block by default', () => {
    const nodes = [makeNode(track('a'), { x: 12, y: 34 }, { label: 'n' })];
    const yaml = buildPipelineForYaml(nodes, [], 'oneshot');
    expect(yaml.nodes['n']).not.toHaveProperty('ui');
  });

  it('emits a rounded ui.position when includeUiPositions=true', () => {
    const nodes = [makeNode(track('a'), { x: 12.4, y: 34.7 }, { label: 'n' })];
    const yaml = buildPipelineForYaml(nodes, [], 'oneshot', { includeUiPositions: true });
    expect(yaml.nodes['n']).toMatchObject({ ui: { position: { x: 12, y: 35 } } });
  });
});

describe('buildPipelineForYaml — integration', () => {
  it('produces a complete pipeline with kind, needs, and params', () => {
    const nodes = [
      makeNode(
        track('src'),
        { x: 0, y: 0 },
        {
          label: 'mic',
          kind: 'audio::null_source',
        }
      ),
      makeNode(
        track('gain'),
        { x: 0, y: 100 },
        {
          label: 'gain',
          kind: 'audio::gain',
          params: { gain_db: 0 },
        }
      ),
      makeNode(
        track('sink'),
        { x: 0, y: 200 },
        {
          label: 'out',
          kind: 'audio::null_sink',
        }
      ),
    ];
    const edges = [makeEdge('e0', 'src', 'gain'), makeEdge('e1', 'gain', 'sink')];
    const yaml = buildPipelineForYaml(nodes, edges, 'dynamic');

    expect(Object.keys(yaml.nodes)).toEqual(['mic', 'gain', 'out']);
    expect(yaml.nodes['mic']).toMatchObject({ kind: 'audio::null_source' });
    expect(yaml.nodes['gain']).toMatchObject({
      kind: 'audio::gain',
      needs: 'mic',
      params: { gain_db: 0 },
    });
    expect(yaml.nodes['out']).toMatchObject({
      kind: 'audio::null_sink',
      needs: 'gain',
    });
  });
});
