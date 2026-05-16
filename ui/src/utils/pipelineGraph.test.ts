// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { Node as RFNode } from '@xyflow/react';
import { load } from 'js-yaml';
import { describe, expect, it } from 'vitest';

import type { Connection, InputPin, NodeState, OutputPin, Pipeline } from '@/types/types';

import {
  buildEdgesFromConnections,
  buildNodeObject,
  describeSlowInputs,
  extractSlowTimeoutDetailsFromNodeState,
  generatePipelineYaml,
  isRecord,
  nodeTypeForKind,
} from './pipelineGraph';

function makeRFNode(id: string, inputs: InputPin[], outputs: OutputPin[]): RFNode {
  return {
    id,
    type: 'configurable',
    position: { x: 0, y: 0 },
    data: { inputs, outputs },
  } as RFNode;
}

function pinOne(name: string): InputPin {
  return { name, accepts_types: ['Any'], cardinality: 'One' };
}

function outPinOne(name: string): OutputPin {
  return { name, produces_type: 'Any', cardinality: 'Broadcast' };
}

function dynamicInputPin(name: string, prefix: string): InputPin {
  return {
    name,
    accepts_types: ['Any'],
    cardinality: { Dynamic: { prefix } },
  };
}

function dynamicOutputPin(name: string, prefix: string): OutputPin {
  return {
    name,
    produces_type: 'Any',
    cardinality: { Dynamic: { prefix } },
  };
}

describe('isRecord', () => {
  it('returns true for plain objects', () => {
    expect(isRecord({})).toBe(true);
    expect(isRecord({ a: 1 })).toBe(true);
  });

  it('returns false for arrays', () => {
    expect(isRecord([])).toBe(false);
    expect(isRecord([1, 2])).toBe(false);
  });

  it('returns false for null and undefined', () => {
    expect(isRecord(null)).toBe(false);
    expect(isRecord(undefined)).toBe(false);
  });

  it('returns false for primitives', () => {
    expect(isRecord('string')).toBe(false);
    expect(isRecord(42)).toBe(false);
    expect(isRecord(true)).toBe(false);
    expect(isRecord(Symbol('x'))).toBe(false);
  });

  it('narrows to Record<string, unknown> on true branch', () => {
    const value: unknown = { foo: 'bar' };
    if (isRecord(value)) {
      expect(value['foo']).toBe('bar');
    } else {
      throw new Error('expected isRecord to narrow');
    }
  });
});

describe('extractSlowTimeoutDetailsFromNodeState', () => {
  it('returns null for null/undefined', () => {
    expect(extractSlowTimeoutDetailsFromNodeState(null)).toBeNull();
    expect(extractSlowTimeoutDetailsFromNodeState(undefined)).toBeNull();
  });

  it('returns null for plain string states like "Running"', () => {
    expect(extractSlowTimeoutDetailsFromNodeState('Running')).toBeNull();
    expect(extractSlowTimeoutDetailsFromNodeState('Initializing')).toBeNull();
  });

  it('returns null for non-Degraded variants', () => {
    const failed: NodeState = { Failed: { reason: 'boom' } };
    const stopped: NodeState = { Stopped: { reason: 'completed' } };
    const recovering: NodeState = { Recovering: { reason: 'r', details: null } };
    expect(extractSlowTimeoutDetailsFromNodeState(failed)).toBeNull();
    expect(extractSlowTimeoutDetailsFromNodeState(stopped)).toBeNull();
    expect(extractSlowTimeoutDetailsFromNodeState(recovering)).toBeNull();
  });

  it('returns null for Degraded with a non slow_input_timeout reason', () => {
    const state: NodeState = {
      Degraded: { reason: 'other_reason', details: { slow_pins: ['a'] } },
    };
    expect(extractSlowTimeoutDetailsFromNodeState(state)).toBeNull();
  });

  it('returns null when Degraded.details is not a record', () => {
    const stateNull: NodeState = {
      Degraded: { reason: 'slow_input_timeout', details: null },
    };
    const stateArray: NodeState = {
      Degraded: { reason: 'slow_input_timeout', details: ['a'] as unknown as NodeState },
    };
    const stateString: NodeState = {
      Degraded: { reason: 'slow_input_timeout', details: 'oops' as unknown as NodeState },
    };
    expect(extractSlowTimeoutDetailsFromNodeState(stateNull)).toBeNull();
    expect(extractSlowTimeoutDetailsFromNodeState(stateArray)).toBeNull();
    expect(extractSlowTimeoutDetailsFromNodeState(stateString)).toBeNull();
  });

  it('returns full details when reason is slow_input_timeout', () => {
    const state: NodeState = {
      Degraded: {
        reason: 'slow_input_timeout',
        details: {
          slow_pins: ['audio_in', 'video_in'],
          newly_slow_pins: ['audio_in'],
          sync_timeout_ms: 250,
        },
      },
    };
    expect(extractSlowTimeoutDetailsFromNodeState(state)).toEqual({
      slowPins: ['audio_in', 'video_in'],
      newlySlowPins: ['audio_in'],
      syncTimeoutMs: 250,
    });
  });

  it('filters non-string entries out of slow_pins and newly_slow_pins', () => {
    const state: NodeState = {
      Degraded: {
        reason: 'slow_input_timeout',
        details: {
          slow_pins: ['a', 1, null, 'b', { x: 1 }],
          newly_slow_pins: [true, 'c', undefined, 'd'],
          sync_timeout_ms: 100,
        },
      },
    };
    expect(extractSlowTimeoutDetailsFromNodeState(state)).toEqual({
      slowPins: ['a', 'b'],
      newlySlowPins: ['c', 'd'],
      syncTimeoutMs: 100,
    });
  });

  it('falls back to empty arrays / null when optional fields are missing or wrongly typed', () => {
    const state: NodeState = {
      Degraded: {
        reason: 'slow_input_timeout',
        details: {},
      },
    };
    expect(extractSlowTimeoutDetailsFromNodeState(state)).toEqual({
      slowPins: [],
      newlySlowPins: [],
      syncTimeoutMs: null,
    });

    const stateWrongTypes: NodeState = {
      Degraded: {
        reason: 'slow_input_timeout',
        details: {
          slow_pins: 'not-an-array',
          newly_slow_pins: { a: 1 },
          sync_timeout_ms: '250',
        },
      },
    };
    expect(extractSlowTimeoutDetailsFromNodeState(stateWrongTypes)).toEqual({
      slowPins: [],
      newlySlowPins: [],
      syncTimeoutMs: null,
    });
  });
});

describe('describeSlowInputs', () => {
  const pipeline: Pipeline = {
    name: null,
    description: null,
    mode: 'dynamic',
    nodes: {
      src_a: { kind: 'audio::source', params: {}, state: null },
      src_b: { kind: 'audio::source', params: {}, state: null },
      mixer: { kind: 'audio::mixer', params: {}, state: null },
    },
    connections: [
      { from_node: 'src_a', from_pin: 'out', to_node: 'mixer', to_pin: 'audio_in' },
      { from_node: 'src_b', from_pin: 'out', to_node: 'mixer', to_pin: 'video_in' },
    ],
  } as unknown as Pipeline;

  it('returns [] when no slow pins are provided', () => {
    expect(describeSlowInputs(pipeline, 'mixer', [])).toEqual([]);
  });

  it('describes inbound edges feeding into slow pins, sorted', () => {
    expect(describeSlowInputs(pipeline, 'mixer', ['video_in', 'audio_in'])).toEqual([
      'src_a.out → audio_in',
      'src_b.out → video_in',
    ]);
  });

  it('ignores slow pins on other nodes', () => {
    expect(describeSlowInputs(pipeline, 'src_a', ['audio_in'])).toEqual([]);
  });
});

describe('nodeTypeForKind', () => {
  it('maps audio::gain to audioGain', () => {
    expect(nodeTypeForKind('audio::gain')).toBe('audioGain');
  });

  it('maps video::compositor to compositor', () => {
    expect(nodeTypeForKind('video::compositor')).toBe('compositor');
  });

  it('falls back to configurable for unknown kinds', () => {
    expect(nodeTypeForKind('audio::resampler')).toBe('configurable');
    expect(nodeTypeForKind('')).toBe('configurable');
    expect(nodeTypeForKind('plugin::native::whisper')).toBe('configurable');
  });
});

describe('buildNodeObject', () => {
  it('returns an RFNode with deterministic id, type, and core fields populated', () => {
    const onParamChange = (): void => {};
    const onConfigChange = (): void => {};

    const node = buildNodeObject({
      nodeName: 'compositor_1',
      apiNode: { kind: 'video::compositor', params: { foo: 'bar' }, state: 'Running' },
      position: { x: 10, y: 20 },
      nodeState: 'Running',
      finalInputs: [pinOne('in')],
      finalOutputs: [outPinOne('out')],
      nodeDef: undefined,
      stableOnParamChange: onParamChange,
      stableOnConfigChange: onConfigChange,
      selectedSessionId: 'session-1',
    });

    expect(node.id).toBe('compositor_1');
    expect(node.type).toBe('compositor');
    expect(node.position).toEqual({ x: 10, y: 20 });
    expect(node.dragHandle).toBe('.drag-handle');

    expect(node.data.label).toBe('compositor_1');
    expect(node.data.kind).toBe('video::compositor');
    expect(node.data.params).toEqual({ foo: 'bar' });
    expect(node.data.inputs).toEqual([pinOne('in')]);
    expect(node.data.outputs).toEqual([outPinOne('out')]);
    expect(node.data.state).toBe('Running');
    expect(node.data.sessionId).toBe('session-1');
    expect(node.data.onParamChange).toBe(onParamChange);
    expect(node.data.onConfigChange).toBe(onConfigChange);
  });

  it('defaults params and sessionId when the api node has no params and no session is selected', () => {
    const node = buildNodeObject({
      nodeName: 'gain_1',
      apiNode: {
        kind: 'audio::gain',
        params: null as unknown as Record<string, unknown>,
        state: null,
      },
      position: { x: 0, y: 0 },
      nodeState: null,
      finalInputs: [],
      finalOutputs: [],
      nodeDef: undefined,
      stableOnParamChange: () => {},
      selectedSessionId: null,
    });

    expect(node.type).toBe('audioGain');
    expect(node.data.params).toEqual({});
    expect(node.data.sessionId).toBeUndefined();
  });

  it('exposes the node definition and bidirectional flag when provided', () => {
    const nodeDef = {
      kind: 'audio::mixer',
      param_schema: { properties: { gain: { type: 'number' } } },
      inputs: [pinOne('in')],
      outputs: [outPinOne('out')],
      categories: ['audio'],
      bidirectional: true,
    } as Parameters<typeof buildNodeObject>[0]['nodeDef'];

    const node = buildNodeObject({
      nodeName: 'mixer_1',
      apiNode: { kind: 'audio::mixer', params: {}, state: null },
      position: { x: 1, y: 2 },
      nodeState: null,
      finalInputs: [pinOne('in')],
      finalOutputs: [outPinOne('out')],
      nodeDef,
      stableOnParamChange: () => {},
      selectedSessionId: null,
    });

    expect(node.data.paramSchema).toBe(nodeDef?.param_schema);
    expect(node.data.nodeDefinition).toBe(nodeDef);
    expect(node.data.definition).toEqual({ bidirectional: true });
  });
});

describe('buildEdgesFromConnections', () => {
  it('returns an empty array for an empty pipeline (no nodes, no connections)', () => {
    expect(buildEdgesFromConnections([], [])).toEqual([]);
  });

  it('returns an empty array when there are nodes but no connections', () => {
    const nodes = [makeRFNode('a', [], [outPinOne('out')]), makeRFNode('b', [pinOne('in')], [])];
    expect(buildEdgesFromConnections([], nodes)).toEqual([]);
  });

  it('builds an edge with a deterministic id for a valid connection', () => {
    const nodes = [makeRFNode('a', [], [outPinOne('out')]), makeRFNode('b', [pinOne('in')], [])];
    const connections: Connection[] = [
      { from_node: 'a', from_pin: 'out', to_node: 'b', to_pin: 'in' },
    ];

    const edges = buildEdgesFromConnections(connections, nodes);
    expect(edges).toHaveLength(1);
    expect(edges[0]).toEqual({
      id: 'a_out-b_in',
      source: 'a',
      sourceHandle: 'out',
      target: 'b',
      targetHandle: 'in',
    });
  });

  it('produces stable ids regardless of input ordering', () => {
    const nodes = [
      makeRFNode('a', [], [outPinOne('out')]),
      makeRFNode('b', [pinOne('in')], [outPinOne('out')]),
      makeRFNode('c', [pinOne('in')], []),
    ];
    const connections: Connection[] = [
      { from_node: 'a', from_pin: 'out', to_node: 'b', to_pin: 'in' },
      { from_node: 'b', from_pin: 'out', to_node: 'c', to_pin: 'in' },
    ];

    const a = buildEdgesFromConnections(connections, nodes);
    const b = buildEdgesFromConnections([...connections].reverse(), [...nodes].reverse());

    const idsA = a.map((e) => e.id).sort();
    const idsB = b.map((e) => e.id).sort();
    expect(idsA).toEqual(idsB);
    expect(idsA).toEqual(['a_out-b_in', 'b_out-c_in']);
  });

  it('filters out connections referencing an unknown source or target node', () => {
    const nodes = [makeRFNode('a', [], [outPinOne('out')]), makeRFNode('b', [pinOne('in')], [])];
    const connections: Connection[] = [
      { from_node: 'a', from_pin: 'out', to_node: 'missing', to_pin: 'in' },
      { from_node: 'missing', from_pin: 'out', to_node: 'b', to_pin: 'in' },
      { from_node: 'a', from_pin: 'out', to_node: 'b', to_pin: 'in' },
    ];

    const edges = buildEdgesFromConnections(connections, nodes);
    expect(edges.map((e) => e.id)).toEqual(['a_out-b_in']);
  });

  it('filters out connections referencing a non-existent pin name on either side', () => {
    const nodes = [makeRFNode('a', [], [outPinOne('out')]), makeRFNode('b', [pinOne('in')], [])];
    const connections: Connection[] = [
      { from_node: 'a', from_pin: 'nope', to_node: 'b', to_pin: 'in' },
      { from_node: 'a', from_pin: 'out', to_node: 'b', to_pin: 'nope' },
      { from_node: 'a', from_pin: 'out', to_node: 'b', to_pin: 'in' },
    ];

    const edges = buildEdgesFromConnections(connections, nodes);
    expect(edges).toHaveLength(1);
    expect(edges[0]?.id).toBe('a_out-b_in');
  });

  it('rejects connections that resolve to Dynamic template pins', () => {
    const nodes = [
      makeRFNode('a', [], [dynamicOutputPin('out', 'out_')]),
      makeRFNode('b', [dynamicInputPin('in', 'in_')], []),
    ];
    const connections: Connection[] = [
      { from_node: 'a', from_pin: 'out', to_node: 'b', to_pin: 'in' },
    ];

    expect(buildEdgesFromConnections(connections, nodes)).toEqual([]);
  });

  it('accepts concrete pins generated from a Dynamic template (different name from the template)', () => {
    const nodes = [
      makeRFNode('a', [], [dynamicOutputPin('out', 'out_'), outPinOne('out_track_a')]),
      makeRFNode('b', [dynamicInputPin('in', 'in_'), pinOne('in_track_a')], []),
    ];
    const connections: Connection[] = [
      { from_node: 'a', from_pin: 'out_track_a', to_node: 'b', to_pin: 'in_track_a' },
    ];

    const edges = buildEdgesFromConnections(connections, nodes);
    expect(edges).toHaveLength(1);
    expect(edges[0]?.id).toBe('a_out_track_a-b_in_track_a');
  });
});

describe('generatePipelineYaml', () => {
  function pipelineOf(
    nodes: Record<string, { kind: string; params?: Record<string, unknown> }>,
    connections: Connection[]
  ): Pipeline {
    return {
      name: null,
      description: null,
      mode: 'dynamic',
      nodes: Object.fromEntries(
        Object.entries(nodes).map(([name, n]) => [
          name,
          { kind: n.kind, params: n.params ?? {}, state: null },
        ])
      ),
      connections,
    } as unknown as Pipeline;
  }

  it('emits an empty nodes mapping for an empty pipeline', () => {
    const yaml = generatePipelineYaml(pipelineOf({}, []), []);
    const parsed = load(yaml) as { nodes: Record<string, unknown> };
    expect(parsed).toEqual({ nodes: {} });
  });

  it('emits each node with kind and omits params when there are none and needs when there are no connections', () => {
    const pipeline = pipelineOf({ a: { kind: 'audio::source' }, b: { kind: 'audio::sink' } }, []);
    const yaml = generatePipelineYaml(pipeline, ['a', 'b']);
    const parsed = load(yaml) as {
      nodes: Record<string, { kind: string; params?: unknown; needs?: unknown }>;
    };
    expect(parsed.nodes['a']).toEqual({ kind: 'audio::source' });
    expect(parsed.nodes['b']).toEqual({ kind: 'audio::sink' });
    expect(parsed.nodes['a']?.params).toBeUndefined();
    expect(parsed.nodes['a']?.needs).toBeUndefined();
  });

  it('emits a single string for `needs` when a node has exactly one upstream', () => {
    const pipeline = pipelineOf({ a: { kind: 'audio::source' }, b: { kind: 'audio::sink' } }, [
      { from_node: 'a', from_pin: 'out', to_node: 'b', to_pin: 'in' },
    ]);
    const yaml = generatePipelineYaml(pipeline, ['a', 'b']);
    const parsed = load(yaml) as { nodes: Record<string, { needs?: unknown }> };
    expect(parsed.nodes['b']?.needs).toBe('a');
  });

  it('emits an array for `needs` when a node has multiple upstreams', () => {
    const pipeline = pipelineOf(
      {
        a: { kind: 'audio::source' },
        b: { kind: 'audio::source' },
        c: { kind: 'audio::mixer' },
      },
      [
        { from_node: 'a', from_pin: 'out', to_node: 'c', to_pin: 'in1' },
        { from_node: 'b', from_pin: 'out', to_node: 'c', to_pin: 'in2' },
      ]
    );
    const yaml = generatePipelineYaml(pipeline, ['a', 'b', 'c']);
    const parsed = load(yaml) as { nodes: Record<string, { needs?: unknown }> };
    expect(parsed.nodes['c']?.needs).toEqual(['a', 'b']);
  });

  it('emits params only when non-empty', () => {
    const pipeline = pipelineOf(
      {
        empty_params: { kind: 'audio::source', params: {} },
        with_params: { kind: 'audio::source', params: { gain: 1.5 } },
      },
      []
    );
    const yaml = generatePipelineYaml(pipeline, ['empty_params', 'with_params']);
    const parsed = load(yaml) as {
      nodes: Record<string, { params?: Record<string, unknown> }>;
    };
    expect(parsed.nodes['empty_params']?.params).toBeUndefined();
    expect(parsed.nodes['with_params']?.params).toEqual({ gain: 1.5 });
  });

  it('skips orderedNames that are not present in the pipeline', () => {
    const pipeline = pipelineOf({ a: { kind: 'audio::source' } }, []);
    const yaml = generatePipelineYaml(pipeline, ['ghost', 'a']);
    const parsed = load(yaml) as { nodes: Record<string, unknown> };
    expect(Object.keys(parsed.nodes)).toEqual(['a']);
  });

  it('round-trips a non-trivial pipeline through dump + parse', () => {
    const pipeline = pipelineOf(
      {
        src: { kind: 'audio::source', params: { rate: 48000 } },
        gain: { kind: 'audio::gain', params: { db: -3 } },
        sink: { kind: 'audio::sink' },
      },
      [
        { from_node: 'src', from_pin: 'out', to_node: 'gain', to_pin: 'in' },
        { from_node: 'gain', from_pin: 'out', to_node: 'sink', to_pin: 'in' },
      ]
    );

    const yaml = generatePipelineYaml(pipeline, ['src', 'gain', 'sink']);
    const parsed = load(yaml) as {
      nodes: Record<string, { kind: string; params?: unknown; needs?: unknown }>;
    };

    expect(parsed).toEqual({
      nodes: {
        src: { kind: 'audio::source', params: { rate: 48000 } },
        gain: { kind: 'audio::gain', params: { db: -3 }, needs: 'src' },
        sink: { kind: 'audio::sink', needs: 'gain' },
      },
    });
  });
});
