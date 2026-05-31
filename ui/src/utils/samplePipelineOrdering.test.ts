// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it } from 'vitest';

import type { SamplePipeline } from '@/types/generated/api-types';

import {
  baseVariantLabel,
  collectSampleFacets,
  compareSamplePipelinesByName,
  formatCapabilityLabel,
  groupSamplePipelinesByScenario,
  matchesSamplePipelineQuery,
  orderSamplePipelinesSystemFirst,
  sampleNeedsHardware,
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
    group: partial.group ?? null,
    variant: partial.variant ?? null,
    category: partial.category ?? null,
    tags: partial.tags ?? [],
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

  it('expands synonyms so abbreviations find derived tags', () => {
    const stt = makePipeline({ id: 'stt', name: 'Whisper', tags: ['speech-to-text'] });
    const tts = makePipeline({ id: 'tts', name: 'Kokoro', tags: ['text-to-speech'] });
    expect(matchesSamplePipelineQuery(stt, 'stt')).toBe(true);
    expect(matchesSamplePipelineQuery(stt, 'transcribe')).toBe(true);
    expect(matchesSamplePipelineQuery(tts, 'tts')).toBe(true);
    expect(matchesSamplePipelineQuery(tts, 'speech synthesis')).toBe(true);
    expect(matchesSamplePipelineQuery(tts, 'stt')).toBe(false);
  });

  it('expands a whole-token prefix of a multi-word synonym', () => {
    const tts = makePipeline({ id: 'tts', name: 'Kokoro', tags: ['text-to-speech'] });
    expect(matchesSamplePipelineQuery(tts, 'synthesis')).toBe(true);
  });

  it('does not expand both video families from a shared token', () => {
    // "video" is a token of the hyphenated derived tags video-encoding and
    // video-decoding, but those are expansion targets only, so querying "video"
    // must not pull a decoder pipeline in via the encode group's synonyms.
    const decoder = makePipeline({ id: 'dec', name: 'AV1 Decode', tags: ['video-decoding'] });
    expect(matchesSamplePipelineQuery(decoder, 'encoder')).toBe(false);
  });

  it('does not let a query that merely contains a short synonym leak in its group', () => {
    const mic = makePipeline({ id: 'mic', name: 'Microphone Capture', tags: ['microphone'] });
    const cam = makePipeline({ id: 'cam', name: 'Webcam PiP', tags: ['webcam'] });
    // "dynamic" contains "mic" and "scam" contains "cam"; neither should expand
    // the microphone/webcam synonym groups.
    expect(matchesSamplePipelineQuery(mic, 'dynamic')).toBe(false);
    expect(matchesSamplePipelineQuery(cam, 'scam')).toBe(false);
    // Genuine prefixes still expand their group.
    expect(matchesSamplePipelineQuery(cam, 'camera')).toBe(true);
  });

  it('requires every query term to match (AND semantics)', () => {
    const sample = makePipeline({
      id: 'hw-encode',
      name: 'VA-API H.264 Colorbars',
      tags: ['video-encoding', 'hardware:vaapi'],
    });
    expect(matchesSamplePipelineQuery(sample, 'vaapi encode')).toBe(true);
    expect(matchesSamplePipelineQuery(sample, 'vaapi audio')).toBe(false);
  });
});

describe('groupSamplePipelinesByScenario', () => {
  const plain = makePipeline({
    id: 'd/colorbars',
    name: 'Colorbars',
    group: 'video-moq-colorbars',
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

  it('collapses same-group samples into one entry with sorted variants', () => {
    const groups = groupSamplePipelinesByScenario([h264, plain, vaapi]);
    expect(groups).toHaveLength(1);
    expect(groups[0].key).toBe('video-moq-colorbars');
    // Canonical (no variant) is the base and comes first.
    expect(groups[0].base).toBe(plain);
    expect(groups[0].variants[0]).toBe(plain);
    expect(groups[0].variants.map((v) => v.variant)).toEqual([null, 'H.264', 'VA-API H.264']);
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
  it('aggregates sorted categories and capabilities, excluding hardware and format tags', () => {
    const facets = collectSampleFacets([
      makePipeline({
        id: 'a',
        category: 'Video Encoding',
        tags: ['hardware:vaapi', 'colorbars', 'codec:vp9'],
      }),
      makePipeline({ id: 'b', category: 'Speech to Text', tags: ['mp4'] }),
    ]);
    expect(facets.categories).toEqual(['Speech to Text', 'Video Encoding']);
    // codec:* and container/transport tags are the variant axis, not facets.
    expect(facets.capabilities).toEqual(['colorbars']);
    expect(facets.hasHardware).toBe(true);
  });

  it('keeps tags as capabilities even when a same-named category is shown', () => {
    // category is a single priority-picked bucket while tags are multi-valued,
    // so a capability chip must survive as a cross-cutting filter (e.g. a
    // sample bucketed as `Video Compositing` may still carry `video-encoding`).
    const facets = collectSampleFacets([
      makePipeline({ id: 'a', category: 'Video Encoding', tags: ['video-encoding', 'codec:vp9'] }),
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
    const facets = collectSampleFacets([makePipeline({ id: 'a', tags: ['mp4'] })]);
    expect(facets.hasHardware).toBe(false);
  });
});

describe('formatCapabilityLabel', () => {
  it('uses curated acronym casing for known tags', () => {
    expect(formatCapabilityLabel('moq')).toBe('MoQ');
    expect(formatCapabilityLabel('mp4')).toBe('MP4');
    expect(formatCapabilityLabel('mse')).toBe('MSE');
    expect(formatCapabilityLabel('rtmp')).toBe('RTMP');
    expect(formatCapabilityLabel('webm')).toBe('WebM');
    expect(formatCapabilityLabel('vad')).toBe('VAD');
  });

  it('falls back to title-casing for other tags', () => {
    expect(formatCapabilityLabel('voice-activity-detection')).toBe('Voice Activity Detection');
    expect(formatCapabilityLabel('colorbars')).toBe('Colorbars');
  });
});

describe('baseVariantLabel', () => {
  it('derives the base label from the output codec tag', () => {
    const colorbars = makePipeline({ id: 'cb', tags: ['codec:vp9', 'moq'] });
    const mixer = makePipeline({ id: 'mix', tags: ['codec:opus', 'mixing'] });
    expect(baseVariantLabel(colorbars)).toBe('VP9');
    expect(baseVariantLabel(mixer)).toBe('Opus');
  });

  it('prefers the video codec when a sample carries both', () => {
    const pip = makePipeline({ id: 'pip', tags: ['codec:opus', 'codec:h264'] });
    expect(baseVariantLabel(pip)).toBe('H.264');
  });

  it('returns null when there is no codec tag to distinguish the base', () => {
    expect(baseVariantLabel(makePipeline({ id: 'x', tags: ['mixing'] }))).toBeNull();
  });
});
