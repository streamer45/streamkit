// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { SamplePipeline } from '@/types/generated/api-types';

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

// A query term joins a synonym group on an exact match, or when it is a
// substring of an entry (>=3 chars, so "transcrib" finds "transcribe"). The
// reverse direction (entry being a substring of the term) is deliberately
// excluded: short entries like "mic"/"cam" would otherwise pull whole groups
// into unrelated queries ("dynamic" → microphone, "scam" → webcam).
function expandTerm(term: string): string[] {
  const expanded = new Set<string>([term]);
  for (const group of SYNONYM_GROUPS) {
    if (group.some((entry) => entry === term || (term.length >= 3 && entry.includes(term)))) {
      for (const entry of group) expanded.add(entry);
    }
  }
  return [...expanded];
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
 * Token + synonym match: every whitespace-separated query term must match the
 * pipeline's searchable text (name, description, id, category, variant, group,
 * tags), where a term matches if it — or any of its synonyms — is a substring.
 */
export function matchesSamplePipelineQuery(pipeline: SamplePipeline, query: string): boolean {
  const normalizedQuery = query.trim().toLowerCase();
  if (!normalizedQuery) return true;

  const haystack = searchableText(pipeline);
  const terms = normalizedQuery.split(/\s+/).filter(Boolean);

  return terms.every((term) => expandTerm(term).some((candidate) => haystack.includes(candidate)));
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

export function sampleNeedsHardware(sample: SamplePipeline): boolean {
  return (sample.tags ?? []).some((tag) => tag.startsWith(HARDWARE_TAG_PREFIX));
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
      } else {
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
