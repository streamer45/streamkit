// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useQuery } from '@tanstack/react-query';

import type { AssetTypeInfo } from '@/types/generated/api-types';
import { getLogger } from '@/utils/logger';

import { fetchApi } from './base';

const logger = getLogger('assetTypes');

export async function listAssetTypes(): Promise<AssetTypeInfo[]> {
  logger.debug('Fetching asset types');

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
  logger.debug('Fetched', types.length, 'asset types');

  return types;
}

export function useAssetTypes() {
  return useQuery({
    queryKey: ['assetTypes'],
    queryFn: listAssetTypes,
    staleTime: 5 * 60 * 1000, // 5 minutes — types change rarely
    refetchOnWindowFocus: false,
  });
}
