// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { AudioCodec, SamplePipeline, VideoCodec } from '@/types/generated/api-types';

import { labelFromKey } from './jsonSchema';

let collator: Intl.Collator | null = null;

function getCollator(): Intl.Collator {
  if (!collator) {
    collator = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });
  }
  return collator;
}

export function compareSamplePipelinesByName(a: SamplePipeline, b: SamplePipeline): number {
  const nameCompare = getCollator().compare(a.name ?? '', b.name ?? '');
  if (nameCompare !== 0) return nameCompare;
  return getCollator().compare(a.id ?? '', b.id ?? '');
}

export function orderSamplePipelinesSystemFirst(pipelines: SamplePipeline[]): SamplePipeline[] {
  const system: SamplePipeline[] = [];
  const user: SamplePipeline[] = [];

  for (const pipeline of pipelines) {
    if (pipeline.is_system) {
      system.push(pipeline);
    } else {
      user.push(pipeline);
    }
  }

  system.sort(compareSamplePipelinesByName);
  user.sort(compareSamplePipelinesByName);

  return [...system, ...user];
}

/**
 * Groups of interchangeable search terms. A query term matches a pipeline if any
 * term in the same group appears in the searchable text, so "stt", "transcribe"
 * and the derived `speech-to-text` tag are all mutually findable.
 */
const SYNONYM_GROUPS: string[][] = [
  ['stt', 'speech-to-text', 'speech to text', 'transcribe', 'transcription', 'transcript', 'asr'],
  ['tts', 'text-to-speech', 'text to speech', 'speech synthesis', 'synthesize', 'voice'],
  ['translate', 'translation', 'translator'],
  ['vad', 'voice-activity-detection', 'voice activity'],
  ['encode', 'encoding', 'encoder', 'video-encoding', 'transcode'],
  ['decode', 'decoding', 'decoder', 'video-decoding'],
  ['compositor', 'compositing', 'composite', 'overlay'],
  ['webcam', 'camera', 'cam'],
  ['mic', 'microphone'],
  ['moq', 'media over quic', 'stream', 'streaming'],
  ['hw', 'hardware', 'gpu', 'accelerated'],
  ['pip', 'picture-in-picture', 'picture in picture'],
  ['av1', 'aom'],
  ['h264', 'avc', 'h.264'],
  ['hevc', 'h265', 'h.265'],
];

// Whether a query term joins a synonym group via one of its entries. Exact
// matches always count. Otherwise the term must be a >=3 char prefix of a whole
// token of a non-hyphenated entry: this lets "transcrib" find "transcribe" and
// "synthesis" find "speech synthesis", while keeping hyphenated derived tags
// (`video-encoding`, `video-decoding`) as expansion targets only — so a shared
// token like "video" no longer pulls both the encode and decode groups. The
// reverse direction (entry being a substring of the term) is excluded too, so
// "dynamic" never reaches "mic".
function termJoinsEntry(entry: string, term: string): boolean {
  if (entry === term) return true;
  if (entry.includes('-')) return false;
  return entry
    .split(/\s+/)
    .some((token) => token === term || (term.length >= 3 && token.startsWith(term)));
}

function expandTerm(term: string): string[] {
  const expanded = new Set<string>([term]);
  for (const group of SYNONYM_GROUPS) {
    if (group.some((entry) => termJoinsEntry(entry, term))) {
      for (const entry of group) expanded.add(entry);
    }
  }
  return [...expanded];
}

/**
 * Expands a query into its per-term synonym candidate lists once, so callers
 * filtering a list of pipelines do not re-scan the synonym groups per pipeline.
 */
export function expandQueryTerms(query: string): string[][] {
  const normalizedQuery = query.trim().toLowerCase();
  if (!normalizedQuery) return [];
  return normalizedQuery.split(/\s+/).filter(Boolean).map(expandTerm);
}

function searchableText(pipeline: SamplePipeline): string {
  return [
    pipeline.name,
    pipeline.description,
    pipeline.id,
    pipeline.category,
    pipeline.variant,
    pipeline.group,
    ...(pipeline.tags ?? []),
  ]
    .filter(Boolean)
    .join(' ')
    .toLowerCase();
}

/**
 * Whether a pipeline matches an already-expanded query (see `expandQueryTerms`).
 * Every term must match — it or one of its synonyms being a substring of the
 * pipeline's searchable text (name, description, id, category, variant, group,
 * tags). An empty term list matches everything.
 */
export function matchesExpandedQuery(pipeline: SamplePipeline, expandedTerms: string[][]): boolean {
  if (expandedTerms.length === 0) return true;
  const haystack = searchableText(pipeline);
  return expandedTerms.every((candidates) => candidates.some((c) => haystack.includes(c)));
}

/** Convenience wrapper that expands `query` and matches a single pipeline. */
export function matchesSamplePipelineQuery(pipeline: SamplePipeline, query: string): boolean {
  return matchesExpandedQuery(pipeline, expandQueryTerms(query));
}

export interface ScenarioGroup {
  /** Stable key shared by all variants (the pipeline `group`, or the id). */
  key: string;
  /** Representative sample used for the card title/description. */
  base: SamplePipeline;
  /** All samples in the group, canonical (no explicit variant) first. */
  variants: SamplePipeline[];
}

function variantSortKey(sample: SamplePipeline): string {
  return (sample.variant ?? '').toLowerCase();
}

/**
 * Collapses samples that share a `group` into a single entry with a variant
 * list, so near-duplicate cards (e.g. the colorbars codec/hardware family)
 * render once with a variant selector. Samples without a group are singletons.
 * Input order of first appearance is preserved.
 */
export function groupSamplePipelinesByScenario(samples: SamplePipeline[]): ScenarioGroup[] {
  const order: string[] = [];
  const byKey = new Map<string, SamplePipeline[]>();

  for (const sample of samples) {
    const key = sample.group && sample.group.length > 0 ? sample.group : sample.id;
    const existing = byKey.get(key);
    if (existing) {
      existing.push(sample);
    } else {
      byKey.set(key, [sample]);
      order.push(key);
    }
  }

  return order.map((key) => {
    const members = byKey.get(key) ?? [];
    const variants = members.slice().sort((a, b) => {
      const aCanonical = a.variant ? 1 : 0;
      const bCanonical = b.variant ? 1 : 0;
      if (aCanonical !== bCanonical) return aCanonical - bCanonical;
      const variantCompare = getCollator().compare(variantSortKey(a), variantSortKey(b));
      if (variantCompare !== 0) return variantCompare;
      return compareSamplePipelinesByName(a, b);
    });
    const base = variants.find((v) => !v.variant) ?? variants[0];
    return { key, base, variants };
  });
}

const HARDWARE_TAG_PREFIX = 'hardware:';
const CODEC_TAG_PREFIX = 'codec:';

// Codec/container/transport tags are surfaced as the variant axis (pills) and
// remain searchable, but are noise in the capability facets — codec is already
// the pill dimension and the transport is implied by the Streaming category.
const FORMAT_FACET_TAGS = new Set(['moq', 'mse', 'rtmp', 'mp4', 'webm']);

// Display labels keyed on the generated codec enums, so adding a codec variant
// in Rust is a TypeScript compile error here until a label is supplied (rather
// than silently falling back to a mangled title-case like "H264").
const VIDEO_CODEC_LABELS: Record<VideoCodec, string> = {
  vp9: 'VP9',
  h264: 'H.264',
  av1: 'AV1',
};
const AUDIO_CODEC_LABELS: Record<AudioCodec, string> = {
  opus: 'Opus',
  aac: 'AAC',
};
const CODEC_LABELS: Record<string, string> = { ...VIDEO_CODEC_LABELS, ...AUDIO_CODEC_LABELS };

// A group's no-variant base shows the codec its siblings vary on; video codecs
// win over audio when a sample carries both (e.g. webcam PiP encodes both).
const BASE_CODEC_PRIORITY: string[] = [
  ...Object.keys(VIDEO_CODEC_LABELS),
  ...Object.keys(AUDIO_CODEC_LABELS),
];

function isFormatFacetTag(tag: string): boolean {
  return tag.startsWith(CODEC_TAG_PREFIX) || FORMAT_FACET_TAGS.has(tag);
}

export function sampleNeedsHardware(sample: SamplePipeline): boolean {
  return (sample.tags ?? []).some((tag) => tag.startsWith(HARDWARE_TAG_PREFIX));
}

// Acronyms / mixed-case names that the generic title-caser would mangle
// ("Moq", "Mp4"). Anything not listed falls back to labelFromKey. Codec labels
// live in the typed CODEC_LABELS maps above, not here.
const CAPABILITY_LABEL_OVERRIDES: Record<string, string> = {
  moq: 'MoQ',
  mp4: 'MP4',
  mse: 'MSE',
  rtmp: 'RTMP',
  webm: 'WebM',
  vad: 'VAD',
};

export function formatCapabilityLabel(tag: string): string {
  return CAPABILITY_LABEL_OVERRIDES[tag] ?? labelFromKey(tag);
}

/**
 * Pill label for a group's no-variant base, derived from its output codec tag
 * (e.g. the software colorbars base reads `VP9`, the Opus mixer base `Opus`),
 * rather than a hardcoded fallback that misreads non-encoding groups.
 */
export function baseVariantLabel(sample: SamplePipeline): string | null {
  const codecs = (sample.tags ?? [])
    .filter((tag) => tag.startsWith(CODEC_TAG_PREFIX))
    .map((tag) => tag.slice(CODEC_TAG_PREFIX.length));
  if (codecs.length === 0) return null;
  const pick = BASE_CODEC_PRIORITY.find((codec) => codecs.includes(codec)) ?? codecs[0];
  return CODEC_LABELS[pick] ?? pick.toUpperCase();
}

export interface SampleFacets {
  categories: string[];
  capabilities: string[];
  hasHardware: boolean;
}

/** Capability tags exclude the `hardware:*` facets, which surface as a toggle. */
export function collectSampleFacets(samples: SamplePipeline[]): SampleFacets {
  const categories = new Set<string>();
  const capabilities = new Set<string>();
  let hasHardware = false;

  for (const sample of samples) {
    if (sample.category) categories.add(sample.category);
    for (const tag of sample.tags ?? []) {
      if (tag.startsWith(HARDWARE_TAG_PREFIX)) {
        hasHardware = true;
      } else if (!isFormatFacetTag(tag)) {
        capabilities.add(tag);
      }
    }
  }

  const sortStrings = (values: Set<string>): string[] =>
    [...values].sort((a, b) => getCollator().compare(a, b));

  return {
    categories: sortStrings(categories),
    capabilities: sortStrings(capabilities),
    hasHardware,
  };
}
