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

export function matchesSamplePipelineQuery(pipeline: SamplePipeline, query: string): boolean {
  const normalizedQuery = query.trim().toLowerCase();
  if (!normalizedQuery) return true;

  const haystack = [pipeline.name, pipeline.description, pipeline.id]
    .filter(Boolean)
    .join(' ')
    .toLowerCase();

  return haystack.includes(normalizedQuery);
}
