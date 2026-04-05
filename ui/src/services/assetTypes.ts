// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Service for discovering registered asset types (core + plugin).
 *
 * The server exposes `GET /api/v1/asset-types` which returns all asset types
 * that are currently available — core types (audio, images, fonts) are always
 * present, and plugin-declared types appear when the declaring plugin is loaded.
 */

import { useQuery } from '@tanstack/react-query';

import type { AssetTypeInfo } from '@/types/generated/api-types';
import { getLogger } from '@/utils/logger';

import { fetchApi } from './base';

const logger = getLogger('assetTypes');

/**
 * Fetches all registered asset types from the server.
 */
export async function listAssetTypes(): Promise<AssetTypeInfo[]> {
  logger.info('Fetching asset types');

  const response = await fetchApi('/api/v1/asset-types', {
    method: 'GET',
  });

  if (!response.ok) {
    const errorText = await response.text();
    logger.error('Failed to fetch asset types:', {
      status: response.status,
      error: errorText,
    });
    throw new Error(`Failed to fetch asset types: ${response.statusText}`);
  }

  const types: AssetTypeInfo[] = await response.json();
  logger.info('Fetched', types.length, 'asset types');

  return types;
}

/**
 * React Query hook for asset type discovery.
 *
 * Fetches once and caches for a long time — asset types only change when
 * plugins are loaded/unloaded, which is rare.
 */
export function useAssetTypes() {
  return useQuery({
    queryKey: ['assetTypes'],
    queryFn: listAssetTypes,
    staleTime: 5 * 60 * 1000, // 5 minutes — types change rarely
    refetchOnWindowFocus: false,
  });
}
