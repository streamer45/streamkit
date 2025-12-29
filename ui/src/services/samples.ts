// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Service for managing sample pipelines
 */

import type { SamplePipeline, SavePipelineRequest } from '@/types/generated/api-types';
import { getLogger } from '@/utils/logger';

import { getApiUrl } from './base';

const logger = getLogger('samples');

/**
 * Lists all available oneshot sample pipelines
 * @returns A promise that resolves to an array of sample pipelines
 */
export async function listSamples(): Promise<SamplePipeline[]> {
  const apiUrl = getApiUrl();
  const endpoint = `${apiUrl}/api/v1/samples/oneshot`;

  logger.info('Fetching sample pipelines from:', endpoint);

  const response = await fetch(endpoint, {
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

/**
 * Lists all available sample pipelines (oneshot + dynamic).
 * @returns A promise that resolves to an array of sample pipelines
 */
export async function listAllSamples(): Promise<SamplePipeline[]> {
  const [oneshot, dynamic] = await Promise.all([listSamples(), listDynamicSamples()]);
  const merged = [...oneshot, ...dynamic];

  // De-dupe by ID (defensive; endpoints should not overlap)
  const seen = new Set<string>();
  return merged.filter((s) => {
    if (seen.has(s.id)) return false;
    seen.add(s.id);
    return true;
  });
}

/**
 * Lists all available dynamic sample pipelines
 * @returns A promise that resolves to an array of dynamic sample pipelines
 */
export async function listDynamicSamples(): Promise<SamplePipeline[]> {
  const apiUrl = getApiUrl();
  const endpoint = `${apiUrl}/api/v1/samples/dynamic`;

  logger.info('Fetching dynamic sample pipelines from:', endpoint);

  const response = await fetch(endpoint, {
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

/**
 * Gets a specific sample pipeline by ID
 * @param id - The sample pipeline ID
 * @returns A promise that resolves to the sample pipeline
 */
export async function getSample(id: string): Promise<SamplePipeline> {
  const apiUrl = getApiUrl();
  const endpoint = `${apiUrl}/api/v1/samples/oneshot/${encodeURIComponent(id)}`;

  logger.info('Fetching sample pipeline:', id);

  const response = await fetch(endpoint, {
    method: 'GET',
    headers: {
      'Content-Type': 'application/json',
    },
  });

  if (!response.ok) {
    const errorText = await response.text();
    logger.error('Failed to fetch sample:', {
      id,
      status: response.status,
      statusText: response.statusText,
      error: errorText,
    });
    throw new Error(`Failed to fetch sample: ${response.statusText}`);
  }

  const sample: SamplePipeline = await response.json();
  logger.info('Fetched sample:', sample.name);

  return sample;
}

/**
 * Saves a new user pipeline
 * @param request - The pipeline data to save
 * @returns A promise that resolves to the created sample pipeline
 */
export async function saveSample(request: SavePipelineRequest): Promise<SamplePipeline> {
  const apiUrl = getApiUrl();
  const endpoint = `${apiUrl}/api/v1/samples/oneshot`;

  logger.info('Saving user pipeline:', request.name);

  const response = await fetch(endpoint, {
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

/**
 * Deletes a user pipeline
 * @param id - The sample pipeline ID to delete
 * @returns A promise that resolves when the deletion is complete
 */
export async function deleteSample(id: string): Promise<void> {
  const apiUrl = getApiUrl();
  const endpoint = `${apiUrl}/api/v1/samples/oneshot/${encodeURIComponent(id)}`;

  logger.info('Deleting user pipeline:', id);

  const response = await fetch(endpoint, {
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
