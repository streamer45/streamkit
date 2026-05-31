// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import type { SamplePipeline } from '@/types/generated/api-types';

import {
  collectSampleFacets,
  compareSamplePipelinesByName,
  groupSamplePipelinesByScenario,
  matchesSamplePipelineQuery,
  orderSamplePipelinesSystemFirst,
  sampleNeedsHardware,
} from './samplePipelineOrdering';

// Mirrors the backend `build_search_terms`: when a fixture does not supply an
// explicit `search_terms`, derive one from the discovery fields so query tests
// exercise the same document the server emits.
function makePipeline(partial: Partial<SamplePipeline> & { id: string }): SamplePipeline {
  const name = partial.name ?? '';
  const description = partial.description ?? '';
  const category = partial.category ?? null;
  const group = partial.group ?? null;
  const variant = partial.variant ?? null;
  const tags = partial.tags ?? [];
  const search_terms =
    partial.search_terms ??
    [name, description, category, group, variant, ...tags]
      .filter((t): t is string => Boolean(t))
      .map((t) => t.toLowerCase());
  return {
    id: partial.id,
    name,
    description,
    yaml: partial.yaml ?? '',
    is_system: partial.is_system ?? false,
    mode: partial.mode ?? 'dynamic',
    is_fragment: partial.is_fragment ?? false,
    group,
    variant,
    canonical: partial.canonical ?? false,
    category,
    tags,
    search_terms,
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

  it('matches a search term substring', () => {
    expect(matchesSamplePipelineQuery(pipeline, 'mp4')).toBe(true);
  });

  it('returns false when the query is not a substring of any search term', () => {
    expect(matchesSamplePipelineQuery(pipeline, 'flac')).toBe(false);
  });

  it('trims query whitespace before matching', () => {
    expect(matchesSamplePipelineQuery(pipeline, '  mp4  ')).toBe(true);
  });

  it('handles missing fields without throwing', () => {
    const sparse = makePipeline({ id: 'x', name: 'x', description: '' });
    expect(matchesSamplePipelineQuery(sparse, 'x')).toBe(true);
    expect(matchesSamplePipelineQuery(sparse, 'absent')).toBe(false);
  });

  it('matches via tags and category', () => {
    const sample = makePipeline({
      id: 'whisper-transcribe',
      name: 'Live Transcription',
      category: 'Speech to Text',
      tags: ['speech-to-text', 'voice-activity-detection'],
    });
    expect(matchesSamplePipelineQuery(sample, 'speech')).toBe(true);
    expect(matchesSamplePipelineQuery(sample, 'voice activity')).toBe(true);
  });

  it('matches authored aliases baked into the resolved search document', () => {
    // Aliases/synonyms (e.g. "stt") live in each sample's authored keywords,
    // which the backend folds into search_terms; the UI does no expansion.
    const stt = makePipeline({
      id: 'stt',
      name: 'Whisper',
      search_terms: ['whisper', 'speech-to-text', 'stt', 'transcribe'],
    });
    expect(matchesSamplePipelineQuery(stt, 'stt')).toBe(true);
    expect(matchesSamplePipelineQuery(stt, 'transcribe')).toBe(true);
    expect(matchesSamplePipelineQuery(stt, 'tts')).toBe(false);
  });

  it('requires every query term to match (AND semantics)', () => {
    const sample = makePipeline({
      id: 'hw-encode',
      name: 'VA-API H.264 Colorbars',
      tags: ['video-encoding', 'hardware:vaapi'],
    });
    expect(matchesSamplePipelineQuery(sample, 'vaapi encoding')).toBe(true);
    expect(matchesSamplePipelineQuery(sample, 'vaapi audio')).toBe(false);
  });
});

describe('groupSamplePipelinesByScenario', () => {
  const plain = makePipeline({
    id: 'd/colorbars',
    name: 'Colorbars',
    group: 'video-moq-colorbars',
    variant: 'Software',
    canonical: true,
  });
  const h264 = makePipeline({
    id: 'd/h264-colorbars',
    name: 'H.264 Colorbars',
    group: 'video-moq-colorbars',
    variant: 'H.264',
  });
  const vaapi = makePipeline({
    id: 'd/vaapi-colorbars',
    name: 'VA-API Colorbars',
    group: 'video-moq-colorbars',
    variant: 'VA-API H.264',
  });

  it('collapses same-group samples into one entry with the canonical member first', () => {
    const groups = groupSamplePipelinesByScenario([h264, plain, vaapi]);
    expect(groups).toHaveLength(1);
    expect(groups[0].key).toBe('video-moq-colorbars');
    // The canonical member is the base and comes first.
    expect(groups[0].base).toBe(plain);
    expect(groups[0].variants[0]).toBe(plain);
    expect(groups[0].variants.map((v) => v.variant)).toEqual(['Software', 'H.264', 'VA-API H.264']);
  });

  it('treats samples without a group as singletons keyed by id', () => {
    const a = makePipeline({ id: 'oneshot/tts', name: 'TTS' });
    const b = makePipeline({ id: 'oneshot/stt', name: 'STT' });
    const groups = groupSamplePipelinesByScenario([a, b]);
    expect(groups).toHaveLength(2);
    expect(groups.map((g) => g.key)).toEqual(['oneshot/tts', 'oneshot/stt']);
  });

  it('preserves first-appearance order of groups', () => {
    const groups = groupSamplePipelinesByScenario([
      makePipeline({ id: 'b', group: 'beta' }),
      makePipeline({ id: 'a', group: 'alpha' }),
      makePipeline({ id: 'b2', group: 'beta' }),
    ]);
    expect(groups.map((g) => g.key)).toEqual(['beta', 'alpha']);
  });
});

describe('sampleNeedsHardware', () => {
  it('detects hardware facet tags', () => {
    expect(sampleNeedsHardware(makePipeline({ id: 'a', tags: ['hardware:vaapi'] }))).toBe(true);
    expect(sampleNeedsHardware(makePipeline({ id: 'b', tags: ['video-encoding'] }))).toBe(false);
  });
});

describe('collectSampleFacets', () => {
  it('aggregates sorted categories and capabilities, excluding hardware tags', () => {
    const facets = collectSampleFacets([
      makePipeline({
        id: 'a',
        category: 'Video Encoding',
        tags: ['hardware:vaapi', 'colorbars'],
      }),
      makePipeline({ id: 'b', category: 'Speech to Text', tags: ['transcription'] }),
    ]);
    expect(facets.categories).toEqual(['Speech to Text', 'Video Encoding']);
    expect(facets.capabilities).toEqual(['colorbars', 'transcription']);
    expect(facets.hasHardware).toBe(true);
  });

  it('keeps tags as capabilities even when a same-named category is shown', () => {
    // category is a single bucket while tags are multi-valued, so a capability
    // chip must survive as a cross-cutting filter (e.g. a sample bucketed as
    // `Video Compositing` may still carry `video-encoding`).
    const facets = collectSampleFacets([
      makePipeline({ id: 'a', category: 'Video Encoding', tags: ['video-encoding'] }),
      makePipeline({
        id: 'b',
        category: 'Video Compositing',
        tags: ['compositing', 'video-encoding'],
      }),
    ]);
    expect(facets.categories).toEqual(['Video Compositing', 'Video Encoding']);
    expect(facets.capabilities).toEqual(['compositing', 'video-encoding']);
  });

  it('reports no hardware when no hardware tags are present', () => {
    const facets = collectSampleFacets([makePipeline({ id: 'a', tags: ['transcription'] })]);
    expect(facets.hasHardware).toBe(false);
  });
});
