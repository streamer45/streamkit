// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import { topoLevelsFromEdges, verticalLayout } from './dag';

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
});
