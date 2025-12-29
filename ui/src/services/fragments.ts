// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import YAML from 'yaml';

import type { SamplePipeline } from '@/types/generated/api-types';

import { saveSample, deleteSample, listAllSamples } from './samples';

export interface FragmentMetadata {
  tags: string[];
  description: string;
}

/**
 * Encode fragment metadata (tags + description) into the description field
 * Format: "tags:tag1,tag2,tag3|Description text"
 */
function encodeDescription(tags: string[], description: string): string {
  if (tags.length === 0) {
    return description;
  }
  return `tags:${tags.join(',')}|${description}`;
}

/**
 * Decode fragment metadata from the description field
 */
function decodeDescription(encoded: string): FragmentMetadata {
  if (!encoded.includes('tags:')) {
    return { tags: [], description: encoded };
  }

  const [tagsPart, ...descParts] = encoded.split('|');
  const description = descParts.join('|'); // Handle | in description

  const tagsMatch = tagsPart.match(/^tags:(.*)$/);
  const tags = tagsMatch
    ? tagsMatch[1]
        .split(',')
        .map((t) => t.trim())
        .filter((t) => t.length > 0)
    : [];

  return { tags, description };
}

export function decodeFragmentMetadata(encodedDescription: string): FragmentMetadata {
  return decodeDescription(encodedDescription);
}

export function samplesToFragments(
  samples: SamplePipeline[]
): Array<SamplePipeline & FragmentMetadata> {
  return samples
    .filter((sample) => sample.is_fragment)
    .map((sample) => {
      const { tags, description } = decodeFragmentMetadata(sample.description);
      return {
        ...sample,
        tags,
        description,
      };
    });
}

/**
 * Convert fragment nodes (with needs dependencies) to YAML string
 * Uses proper pipeline format: nodes with needs field
 */
export function fragmentToYaml(
  nodes: Record<
    string,
    { kind: string; params?: Record<string, unknown>; needs?: string | string[] }
  >
): string {
  const fragmentData = {
    nodes,
  };
  return YAML.stringify(fragmentData);
}

/**
 * Parse fragment YAML back to nodes
 */
export function yamlToFragment(yaml: string): {
  nodes: Record<
    string,
    { kind: string; params?: Record<string, unknown>; needs?: string | string[] }
  >;
} {
  const parsed = YAML.parse(yaml);
  return {
    nodes: parsed.nodes || {},
  };
}

/**
 * Save a fragment as a sample
 */
export async function saveFragment(
  name: string,
  description: string,
  tags: string[],
  nodes: Record<
    string,
    { kind: string; params?: Record<string, unknown>; needs?: string | string[] }
  >
): Promise<SamplePipeline> {
  const yaml = fragmentToYaml(nodes);
  const encodedDescription = encodeDescription(tags, description);

  return saveSample({
    name,
    description: encodedDescription,
    yaml,
    overwrite: false,
    is_fragment: true,
  });
}

/**
 * Delete a fragment
 */
export async function deleteFragment(id: string): Promise<void> {
  return deleteSample(id);
}

/**
 * List all fragments (filters samples to only return fragments)
 */
export async function listFragments(): Promise<Array<SamplePipeline & FragmentMetadata>> {
  const samples = await listAllSamples();

  return samplesToFragments(samples);
}
