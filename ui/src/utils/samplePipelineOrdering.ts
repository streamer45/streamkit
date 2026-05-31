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

/** Splits a free-text query into lowercased, whitespace-delimited tokens. */
export function tokenizeQuery(query: string): string[] {
  return query.trim().toLowerCase().split(/\s+/).filter(Boolean);
}

/** Flattens a pipeline's backend-resolved `search_terms` into one searchable string. */
export function buildSearchHaystack(pipeline: SamplePipeline): string {
  return (pipeline.search_terms ?? []).join(' ');
}

/** Every token must be a substring of `haystack`; an empty token list matches everything. */
export function haystackMatchesTokens(haystack: string, tokens: string[]): boolean {
  if (tokens.length === 0) return true;
  return tokens.every((token) => haystack.includes(token));
}

/**
 * Whether a pipeline matches a tokenized query, against the backend-resolved
 * `search_terms` document (name, description, id, category, tags, authored
 * keywords, node kinds). Synonyms/aliases live in each sample's authored
 * `keywords`, not in a UI-side table, so the UI does no semantic expansion.
 */
export function matchesQueryTokens(pipeline: SamplePipeline, tokens: string[]): boolean {
  return haystackMatchesTokens(buildSearchHaystack(pipeline), tokens);
}

/** Convenience wrapper that tokenizes `query` and matches a single pipeline. */
export function matchesSamplePipelineQuery(pipeline: SamplePipeline, query: string): boolean {
  return matchesQueryTokens(pipeline, tokenizeQuery(query));
}

export interface ScenarioGroup {
  /** Stable key shared by all variants (the pipeline `group`, or the id). */
  key: string;
  /** Canonical member supplying the card title/description. */
  base: SamplePipeline;
  /** All samples in the group, canonical member first. */
  variants: SamplePipeline[];
}

function variantSortKey(sample: SamplePipeline): string {
  return (sample.variant ?? '').toLowerCase();
}

/**
 * Collapses samples that share a `group` into a single entry with a variant
 * list, so near-duplicate cards (e.g. the colorbars codec/hardware family)
 * render once with a variant selector. Samples without a group are singletons.
 * Group ordering is the caller's responsibility (the picker sorts by name).
 */
export function groupSamplePipelinesByScenario(samples: SamplePipeline[]): ScenarioGroup[] {
  const byKey = new Map<string, SamplePipeline[]>();

  for (const sample of samples) {
    const key = sample.group && sample.group.length > 0 ? sample.group : sample.id;
    const existing = byKey.get(key);
    if (existing) {
      existing.push(sample);
    } else {
      byKey.set(key, [sample]);
    }
  }

  return Array.from(byKey, ([key, members]) => {
    const variants = members.slice().sort((a, b) => {
      const aCanonical = a.canonical ? 0 : 1;
      const bCanonical = b.canonical ? 0 : 1;
      if (aCanonical !== bCanonical) return aCanonical - bCanonical;
      const variantCompare = getCollator().compare(variantSortKey(a), variantSortKey(b));
      if (variantCompare !== 0) return variantCompare;
      return compareSamplePipelinesByName(a, b);
    });
    const base = variants.find((v) => v.canonical) ?? variants[0];
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
