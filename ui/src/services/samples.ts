// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { SamplePipeline, SavePipelineRequest } from '@/types/generated/api-types';
import { getLogger } from '@/utils/logger';

import { fetchApi } from './base';

const logger = getLogger('samples');

export async function listSamples(): Promise<SamplePipeline[]> {
  logger.info('Fetching sample pipelines');

  const response = await fetchApi('/api/v1/samples/oneshot', {
    method: 'GET',
    headers: {
      'Content-Type': 'application/json',
    },
  });

  if (!response.ok) {
    const errorText = await response.text();
    logger.error('Failed to fetch samples:', {
      status: response.status,
      statusText: response.statusText,
      error: errorText,
    });
    throw new Error(`Failed to fetch samples: ${response.statusText}`);
  }

  const samples: SamplePipeline[] = await response.json();
  logger.info('Fetched', samples.length, 'sample pipelines');

  return samples;
}

export async function listAllSamples(): Promise<SamplePipeline[]> {
  const [oneshot, dynamic] = await Promise.all([listSamples(), listDynamicSamples()]);
  const merged = [...oneshot, ...dynamic];

  const seen = new Set<string>();
  return merged.filter((s) => {
    if (seen.has(s.id)) return false;
    seen.add(s.id);
    return true;
  });
}

export async function listDynamicSamples(): Promise<SamplePipeline[]> {
  logger.info('Fetching dynamic sample pipelines');

  const response = await fetchApi('/api/v1/samples/dynamic', {
    method: 'GET',
    headers: {
      'Content-Type': 'application/json',
    },
  });

  if (!response.ok) {
    const errorText = await response.text();
    logger.error('Failed to fetch dynamic samples:', {
      status: response.status,
      statusText: response.statusText,
      error: errorText,
    });
    throw new Error(`Failed to fetch dynamic samples: ${response.statusText}`);
  }

  const samples: SamplePipeline[] = await response.json();
  logger.info('Fetched', samples.length, 'dynamic sample pipelines');

  return samples;
}

export async function saveSample(request: SavePipelineRequest): Promise<SamplePipeline> {
  logger.info('Saving user pipeline:', request.name);

  const response = await fetchApi('/api/v1/samples/oneshot', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(request),
  });

  if (!response.ok) {
    const errorText = await response.text();
    logger.error('Failed to save sample:', {
      name: request.name,
      status: response.status,
      statusText: response.statusText,
      error: errorText,
    });
    throw new Error(
      `Failed to save sample (${response.status}): ${errorText || response.statusText}`
    );
  }

  const sample: SamplePipeline = await response.json();
  logger.info('Saved sample:', sample.name);

  return sample;
}

export async function deleteSample(id: string): Promise<void> {
  logger.info('Deleting user pipeline:', id);

  const response = await fetchApi(`/api/v1/samples/oneshot/${encodeURIComponent(id)}`, {
    method: 'DELETE',
  });

  if (!response.ok) {
    const errorText = await response.text();
    logger.error('Failed to delete sample:', {
      id,
      status: response.status,
      statusText: response.statusText,
      error: errorText,
    });
    throw new Error(`Failed to delete sample: ${errorText || response.statusText}`);
  }

  logger.info('Deleted sample:', id);
}
