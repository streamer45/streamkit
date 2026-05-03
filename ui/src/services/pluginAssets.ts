// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useMutation, useQueryClient } from '@tanstack/react-query';

import type { PluginAsset } from '@/types/generated/api-types';
import { getLogger } from '@/utils/logger';

import { fetchApi } from './base';

const logger = getLogger('pluginAssets');

export async function listPluginAssets(typeId: string): Promise<PluginAsset[]> {
  logger.debug('Fetching plugin assets:', typeId);

  const response = await fetchApi(`/api/v1/assets/plugin/${encodeURIComponent(typeId)}`, {
    method: 'GET',
  });

  if (!response.ok) {
    const errorText = await response.text();
    logger.error('Failed to fetch plugin assets:', {
      typeId,
      status: response.status,
      error: errorText,
    });
    throw new Error(`Failed to fetch ${typeId} assets: ${response.statusText}`);
  }

  const assets: PluginAsset[] = await response.json();
  logger.debug('Fetched', assets.length, typeId, 'assets');

  return assets;
}

export async function uploadPluginAsset(typeId: string, file: File): Promise<PluginAsset> {
  logger.info('Uploading plugin asset:', typeId, file.name);

  const formData = new FormData();
  formData.append('file', file);

  const response = await fetchApi(`/api/v1/assets/plugin/${encodeURIComponent(typeId)}`, {
    method: 'POST',
    body: formData,
  });

  if (!response.ok) {
    const errorText = await response.text();
    logger.error('Failed to upload plugin asset:', {
      typeId,
      fileName: file.name,
      status: response.status,
      error: errorText,
    });
    throw new Error(`Failed to upload ${typeId} asset: ${errorText || response.statusText}`);
  }

  const asset: PluginAsset = await response.json();
  logger.info('Uploaded plugin asset:', asset.name);

  return asset;
}

export async function deletePluginAsset(typeId: string, id: string): Promise<void> {
  logger.info('Deleting plugin asset:', typeId, id);

  const response = await fetchApi(
    `/api/v1/assets/plugin/${encodeURIComponent(typeId)}/${encodeURIComponent(id)}`,
    { method: 'DELETE' }
  );

  if (!response.ok) {
    const errorText = await response.text();
    logger.error('Failed to delete plugin asset:', {
      typeId,
      id,
      status: response.status,
      error: errorText,
    });
    throw new Error(`Failed to delete ${typeId} asset: ${errorText || response.statusText}`);
  }

  logger.info('Deleted plugin asset:', id);
}

export function useUploadPluginAsset(typeId: string) {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (file: File) => {
      if (!typeId) {
        return Promise.reject(new Error('No plugin asset type selected'));
      }
      return uploadPluginAsset(typeId, file);
    },
    onSuccess: () => {
      if (typeId) {
        queryClient.invalidateQueries({ queryKey: ['pluginAssets', typeId] });
      }
    },
  });
}
