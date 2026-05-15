// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import type { Pipeline } from '@/types/types';

import {
  wouldCreateCycle,
  topoLevelsFromEdges,
  topoLevelsFromPipeline,
  orderedNamesFromLevels,
  verticalLayout,
} from './dag';
import type { SimpleEdge } from './dag';

describe('wouldCreateCycle', () => {
  it('returns false when no cycle is created', () => {
    const nodes = ['A', 'B', 'C'];
    const edges: SimpleEdge[] = [{ source: 'A', target: 'B' }];
    expect(wouldCreateCycle(nodes, edges, { source: 'B', target: 'C' })).toBe(false);
  });

  it('returns true when a cycle is created', () => {
    const nodes = ['A', 'B', 'C'];
    const edges: SimpleEdge[] = [
      { source: 'A', target: 'B' },
      { source: 'B', target: 'C' },
    ];
    expect(wouldCreateCycle(nodes, edges, { source: 'C', target: 'A' })).toBe(true);
  });

  it('returns false for self-loop on bidirectional node', () => {
    const nodes = ['A', 'B'];
    const edges: SimpleEdge[] = [{ source: 'A', target: 'B' }];
    expect(wouldCreateCycle(nodes, edges, { source: 'B', target: 'A' }, ['A'])).toBe(false);
  });

  it('returns false for an empty graph', () => {
    expect(wouldCreateCycle(['A', 'B'], [], { source: 'A', target: 'B' })).toBe(false);
  });

  it('handles edges referencing unknown nodes gracefully', () => {
    const nodes = ['A', 'B'];
    const edges: SimpleEdge[] = [{ source: 'X', target: 'Y' }];
    expect(wouldCreateCycle(nodes, edges, { source: 'A', target: 'B' })).toBe(false);
  });
});

describe('topoLevelsFromEdges', () => {
  it('assigns all nodes to level 0 when there are no edges', () => {
    const result = topoLevelsFromEdges(['A', 'B', 'C'], []);
    expect(result.sortedLevels).toEqual([0]);
    expect(result.levels[0]).toEqual(expect.arrayContaining(['A', 'B', 'C']));
  });

  it('assigns correct levels for a linear chain', () => {
    const result = topoLevelsFromEdges(
      ['A', 'B', 'C'],
      [
        { source: 'A', target: 'B' },
        { source: 'B', target: 'C' },
      ]
    );
    expect(result.levelByNode['A']).toBe(0);
    expect(result.levelByNode['B']).toBe(1);
    expect(result.levelByNode['C']).toBe(2);
    expect(result.sortedLevels).toEqual([0, 1, 2]);
  });

  it('assigns correct levels for a diamond graph', () => {
    const result = topoLevelsFromEdges(
      ['A', 'B', 'C', 'D'],
      [
        { source: 'A', target: 'B' },
        { source: 'A', target: 'C' },
        { source: 'B', target: 'D' },
        { source: 'C', target: 'D' },
      ]
    );
    expect(result.levelByNode['A']).toBe(0);
    expect(result.levelByNode['B']).toBe(1);
    expect(result.levelByNode['C']).toBe(1);
    expect(result.levelByNode['D']).toBe(2);
  });

  it('handles a single node', () => {
    const result = topoLevelsFromEdges(['X'], []);
    expect(result.levels[0]).toEqual(['X']);
    expect(result.levelByNode['X']).toBe(0);
  });
});

describe('topoLevelsFromPipeline', () => {
  it('derives levels from a pipeline with connections', () => {
    const pipeline = {
      nodes: { src: {}, dst: {} },
      connections: [{ from_node: 'src', to_node: 'dst', from_pin: 'out', to_pin: 'in' }],
    } as unknown as Pipeline;

    const result = topoLevelsFromPipeline(pipeline);
    expect(result.levelByNode['src']).toBe(0);
    expect(result.levelByNode['dst']).toBe(1);
  });
});

describe('orderedNamesFromLevels', () => {
  it('returns nodes ordered by level', () => {
    const levels = { 0: ['A', 'B'], 1: ['C'], 2: ['D', 'E'] };
    const sorted = [0, 1, 2];
    expect(orderedNamesFromLevels(levels, sorted)).toEqual(['A', 'B', 'C', 'D', 'E']);
  });

  it('returns empty for empty levels', () => {
    expect(orderedNamesFromLevels({}, [])).toEqual([]);
  });
});

describe('verticalLayout', () => {
  it('keeps the primary branch aligned after forks', () => {
    const nodeIds = [
      'resample_for_stt',
      'whisper_stt',
      'stt_telemetry_out',
      'helsinki_translate',
      'translate_telemetry_out',
      'piper_tts',
    ];

    const edges = [
      { source: 'resample_for_stt', target: 'whisper_stt' },
      { source: 'whisper_stt', target: 'helsinki_translate' },
      { source: 'whisper_stt', target: 'stt_telemetry_out' },
      { source: 'helsinki_translate', target: 'piper_tts' },
      { source: 'helsinki_translate', target: 'translate_telemetry_out' },
    ];

    const { levels, sortedLevels } = topoLevelsFromEdges(nodeIds, edges);
    const positions = verticalLayout(levels, sortedLevels, {
      nodeWidth: 100,
      nodeHeight: 50,
      hGap: 40,
      vGap: 30,
      edges,
    });

    expect(positions['whisper_stt']?.x).toBe(positions['helsinki_translate']?.x);
    expect(positions['helsinki_translate']?.x).toBe(positions['piper_tts']?.x);
    expect(positions['stt_telemetry_out']?.x).not.toBe(positions['whisper_stt']?.x);
  });

  it('keeps sibling spacing constant within a level', () => {
    const nodeIds = [
      'root',
      'main_1',
      'main_2',
      'main_3',
      'main_4',
      'main_5',
      'main_next',
      't1',
      't2',
      't3',
      't4',
      't5',
    ];

    const edges = [
      { source: 'root', target: 'main_1' },
      { source: 'main_1', target: 'main_2' },
      { source: 'main_2', target: 'main_3' },
      { source: 'main_3', target: 'main_4' },
      { source: 'main_4', target: 'main_5' },
      { source: 'main_5', target: 'main_next' },

      // Telemetry-like sink branches to inflate lane indices upstream
      { source: 'main_1', target: 't1' },
      { source: 'main_2', target: 't2' },
      { source: 'main_3', target: 't3' },
      { source: 'main_4', target: 't4' },
      { source: 'main_5', target: 't5' },
    ];

    const { levels, sortedLevels } = topoLevelsFromEdges(nodeIds, edges);
    const nodeWidth = 100;
    const hGap = 40;
    const spacing = nodeWidth + hGap;

    const positions = verticalLayout(levels, sortedLevels, {
      nodeWidth,
      nodeHeight: 50,
      hGap,
      vGap: 30,
      edges,
    });

    // The final fork (main_5 -> main_next and t5) should not leave a large horizontal gap.
    const dx = Math.abs((positions['main_next']?.x ?? 0) - (positions['t5']?.x ?? 0));
    expect(dx).toBe(spacing);
  });

  it('uses centered layout when there are no edges', () => {
    const levels = { 0: ['A', 'B', 'C'] };
    const positions = verticalLayout(levels, [0], {
      nodeWidth: 100,
      nodeHeight: 50,
      hGap: 40,
    });

    expect(positions['A']).toBeDefined();
    expect(positions['B']).toBeDefined();
    expect(positions['C']).toBeDefined();
    expect(positions['A']!.y).toBe(positions['B']!.y);
    expect(positions['B']!.y).toBe(positions['C']!.y);
  });

  it('uses default options when none are provided', () => {
    const levels = { 0: ['A'], 1: ['B'] };
    const positions = verticalLayout(levels, [0, 1]);

    expect(positions['A']).toBeDefined();
    expect(positions['B']).toBeDefined();
    expect(positions['A']!.y).toBeLessThan(positions['B']!.y);
  });

  it('respects per-node heights for vertical spacing', () => {
    const nodeIds = ['A', 'B', 'C'];
    const edges: SimpleEdge[] = [
      { source: 'A', target: 'B' },
      { source: 'B', target: 'C' },
    ];
    const { levels, sortedLevels } = topoLevelsFromEdges(nodeIds, edges);

    const positionsUniform = verticalLayout(levels, sortedLevels, {
      nodeHeight: 50,
      vGap: 30,
      edges,
    });
    const positionsTall = verticalLayout(levels, sortedLevels, {
      nodeHeight: 50,
      vGap: 30,
      heights: { A: 200 },
      edges,
    });

    const gapUniform = (positionsUniform['B']?.y ?? 0) - (positionsUniform['A']?.y ?? 0);
    const gapTall = (positionsTall['B']?.y ?? 0) - (positionsTall['A']?.y ?? 0);

    expect(gapTall).toBeGreaterThan(gapUniform);
  });
});
