// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import type { SamplePipeline } from '@/types/generated/api-types';

import {
  compareSamplePipelinesByName,
  matchesSamplePipelineQuery,
  orderSamplePipelinesSystemFirst,
} from './samplePipelineOrdering';

function makePipeline(partial: Partial<SamplePipeline> & { id: string }): SamplePipeline {
  return {
    id: partial.id,
    name: partial.name ?? '',
    description: partial.description ?? '',
    yaml: partial.yaml ?? '',
    is_system: partial.is_system ?? false,
    mode: partial.mode ?? 'dynamic',
    is_fragment: partial.is_fragment ?? false,
  };
}

describe('compareSamplePipelinesByName', () => {
  it('sorts case-insensitively by name', () => {
    const a = makePipeline({ id: '1', name: 'alpha' });
    const b = makePipeline({ id: '2', name: 'Bravo' });
    expect(compareSamplePipelinesByName(a, b)).toBeLessThan(0);
    expect(compareSamplePipelinesByName(b, a)).toBeGreaterThan(0);
  });

  it('sorts numerically embedded in names (Intl numeric collation)', () => {
    const a = makePipeline({ id: '1', name: 'step2' });
    const b = makePipeline({ id: '2', name: 'step10' });
    expect(compareSamplePipelinesByName(a, b)).toBeLessThan(0);
  });

  it('returns 0 when names and ids are equal', () => {
    const a = makePipeline({ id: 'same', name: 'same' });
    const b = makePipeline({ id: 'same', name: 'same' });
    expect(compareSamplePipelinesByName(a, b)).toBe(0);
  });

  it('breaks name ties with id comparison', () => {
    const a = makePipeline({ id: 'a', name: 'same' });
    const b = makePipeline({ id: 'b', name: 'same' });
    expect(compareSamplePipelinesByName(a, b)).toBeLessThan(0);
    expect(compareSamplePipelinesByName(b, a)).toBeGreaterThan(0);
  });
});

describe('orderSamplePipelinesSystemFirst', () => {
  it('places system pipelines before user pipelines', () => {
    const input: SamplePipeline[] = [
      makePipeline({ id: 'u1', name: 'aaa', is_system: false }),
      makePipeline({ id: 's1', name: 'zzz', is_system: true }),
    ];
    const result = orderSamplePipelinesSystemFirst(input);
    expect(result.map((p) => p.id)).toEqual(['s1', 'u1']);
  });

  it('sorts within each group by name (system first, then user)', () => {
    const input: SamplePipeline[] = [
      makePipeline({ id: 'u2', name: 'user-bravo', is_system: false }),
      makePipeline({ id: 's2', name: 'sys-bravo', is_system: true }),
      makePipeline({ id: 'u1', name: 'user-alpha', is_system: false }),
      makePipeline({ id: 's1', name: 'sys-alpha', is_system: true }),
    ];
    const result = orderSamplePipelinesSystemFirst(input);
    expect(result.map((p) => p.id)).toEqual(['s1', 's2', 'u1', 'u2']);
  });

  it('returns an empty array for empty input', () => {
    expect(orderSamplePipelinesSystemFirst([])).toEqual([]);
  });

  it('does not mutate the input array', () => {
    const a = makePipeline({ id: 'u1', name: 'b', is_system: false });
    const b = makePipeline({ id: 's1', name: 'a', is_system: true });
    const input = [a, b];
    const snapshot = [...input];
    orderSamplePipelinesSystemFirst(input);
    expect(input).toEqual(snapshot);
  });
});

describe('matchesSamplePipelineQuery', () => {
  const pipeline = makePipeline({
    id: 'transcode-mp4',
    name: 'Transcode MP4',
    description: 'Convert input to MP4 using FFmpeg',
  });

  it('matches any pipeline when the query is empty', () => {
    expect(matchesSamplePipelineQuery(pipeline, '')).toBe(true);
  });

  it('matches any pipeline when the query is whitespace-only', () => {
    expect(matchesSamplePipelineQuery(pipeline, '   \t\n')).toBe(true);
  });

  it('matches by name (case-insensitive)', () => {
    expect(matchesSamplePipelineQuery(pipeline, 'transcode')).toBe(true);
    expect(matchesSamplePipelineQuery(pipeline, 'TRANSCODE')).toBe(true);
  });

  it('matches by description', () => {
    expect(matchesSamplePipelineQuery(pipeline, 'ffmpeg')).toBe(true);
  });

  it('matches by id', () => {
    expect(matchesSamplePipelineQuery(pipeline, 'mp4')).toBe(true);
  });

  it('returns false when the query is not a substring of any field', () => {
    expect(matchesSamplePipelineQuery(pipeline, 'flac')).toBe(false);
  });

  it('trims query whitespace before matching', () => {
    expect(matchesSamplePipelineQuery(pipeline, '  mp4  ')).toBe(true);
  });

  it('handles missing fields without throwing', () => {
    const sparse = makePipeline({ id: 'x', name: '', description: '' });
    expect(matchesSamplePipelineQuery(sparse, 'x')).toBe(true);
    expect(matchesSamplePipelineQuery(sparse, 'absent')).toBe(false);
  });
});
