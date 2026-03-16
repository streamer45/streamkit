// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import type { Pipeline } from '@/types/types';

import { computeTopoKey } from './pipelineGraph';

const makePipeline = (
  nodes: Record<string, { kind: string; params?: Record<string, unknown> }>,
  connections: Array<{ from_node: string; from_pin: string; to_node: string; to_pin: string }> = []
): Pipeline =>
  ({
    nodes: Object.fromEntries(
      Object.entries(nodes).map(([name, { kind, params }]) => [
        name,
        { kind, params: params ?? {}, state: null },
      ])
    ),
    connections,
  }) as unknown as Pipeline;

describe('computeTopoKey', () => {
  it('returns empty string for null/undefined pipeline', () => {
    expect(computeTopoKey(null, 'session-1')).toBe('');
    expect(computeTopoKey(undefined, 'session-1')).toBe('');
  });

  it('returns empty string for null pipeline even with null sessionId', () => {
    expect(computeTopoKey(null, null)).toBe('');
  });

  it('produces different keys for different sessions with identical topology', () => {
    const pipeline = makePipeline({ gain: { kind: 'audio::gain' } });
    const keyA = computeTopoKey(pipeline, 'session-A');
    const keyB = computeTopoKey(pipeline, 'session-B');

    expect(keyA).not.toBe(keyB);
  });

  it('produces the same key for the same session and topology', () => {
    const pipeline1 = makePipeline({ gain: { kind: 'audio::gain' } });
    const pipeline2 = makePipeline({ gain: { kind: 'audio::gain' } });
    const key1 = computeTopoKey(pipeline1, 'session-1');
    const key2 = computeTopoKey(pipeline2, 'session-1');

    expect(key1).toBe(key2);
  });

  it('ignores param differences (same topology, different params)', () => {
    const pipeline1 = makePipeline({ gain: { kind: 'audio::gain', params: { volume: 0.5 } } });
    const pipeline2 = makePipeline({ gain: { kind: 'audio::gain', params: { volume: 0.9 } } });
    const key1 = computeTopoKey(pipeline1, 'session-1');
    const key2 = computeTopoKey(pipeline2, 'session-1');

    expect(key1).toBe(key2);
  });

  it('changes when a node is added', () => {
    const pipeline1 = makePipeline({ gain: { kind: 'audio::gain' } });
    const pipeline2 = makePipeline({
      gain: { kind: 'audio::gain' },
      comp: { kind: 'video::compositor' },
    });
    const key1 = computeTopoKey(pipeline1, 'session-1');
    const key2 = computeTopoKey(pipeline2, 'session-1');

    expect(key1).not.toBe(key2);
  });

  it('changes when a connection is added', () => {
    const nodes = { source: { kind: 'audio::gain' }, sink: { kind: 'audio::gain' } };
    const pipeline1 = makePipeline(nodes);
    const pipeline2 = makePipeline(nodes, [
      { from_node: 'source', from_pin: 'out', to_node: 'sink', to_pin: 'in' },
    ]);
    const key1 = computeTopoKey(pipeline1, 'session-1');
    const key2 = computeTopoKey(pipeline2, 'session-1');

    expect(key1).not.toBe(key2);
  });

  it('is order-independent for nodes and connections', () => {
    const pipeline1 = makePipeline(
      { b: { kind: 'audio::gain' }, a: { kind: 'video::compositor' } },
      [
        { from_node: 'b', from_pin: 'out', to_node: 'a', to_pin: 'in' },
        { from_node: 'a', from_pin: 'out', to_node: 'b', to_pin: 'in' },
      ]
    );
    // Same nodes and connections, but in different insertion order
    const pipeline2 = makePipeline(
      { a: { kind: 'video::compositor' }, b: { kind: 'audio::gain' } },
      [
        { from_node: 'a', from_pin: 'out', to_node: 'b', to_pin: 'in' },
        { from_node: 'b', from_pin: 'out', to_node: 'a', to_pin: 'in' },
      ]
    );
    const key1 = computeTopoKey(pipeline1, 'session-1');
    const key2 = computeTopoKey(pipeline2, 'session-1');

    expect(key1).toBe(key2);
  });
});
