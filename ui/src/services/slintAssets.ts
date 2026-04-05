// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Service for managing Slint assets.
 *
 * The backend API is being added in a parallel session. These hooks call the
 * expected endpoints so they'll work once the backend lands.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';

import { getLogger } from '@/utils/logger';

import { fetchApi } from './base';

const logger = getLogger('slintAssets');

// Temporary type until backend SlintAsset is generated
export interface SlintAsset {
  id: string;
  name: string;
  path: string;
  format: string; // always "slint"
  size_bytes: number;
  is_system: boolean;
}

/**
 * Lists all available Slint assets (system + user)
 */
export async function listSlintAssets(): Promise<SlintAsset[]> {
  logger.info('Fetching slint assets');

  const response = await fetchApi('/api/v1/assets/slint', {
    method: 'GET',
  });

  if (!response.ok) {
    const errorText = await response.text();
    logger.error('Failed to fetch slint assets:', {
      status: response.status,
      statusText: response.statusText,
      error: errorText,
    });
    throw new Error(`Failed to fetch slint assets: ${response.statusText}`);
  }

  const assets: SlintAsset[] = await response.json();
  logger.info('Fetched', assets.length, 'slint assets');

  return assets;
}

/**
 * Uploads a new Slint asset
 * @param file - The .slint file to upload
 */
export async function uploadSlintAsset(file: File): Promise<SlintAsset> {
  logger.info('Uploading slint asset:', file.name);

  const formData = new FormData();
  formData.append('file', file);

  const response = await fetchApi('/api/v1/assets/slint', {
    method: 'POST',
    body: formData,
  });

  if (!response.ok) {
    const errorText = await response.text();
    logger.error('Failed to upload slint asset:', {
      fileName: file.name,
      status: response.status,
      statusText: response.statusText,
      error: errorText,
    });
    throw new Error(`Failed to upload slint asset: ${errorText || response.statusText}`);
  }

  const asset: SlintAsset = await response.json();
  logger.info('Uploaded slint asset:', asset.name);

  return asset;
}

/**
 * Deletes a Slint asset by ID
 * @param id - The asset ID to delete
 */
export async function deleteSlintAsset(id: string): Promise<void> {
  logger.info('Deleting slint asset:', id);

  const response = await fetchApi(`/api/v1/assets/slint/${encodeURIComponent(id)}`, {
    method: 'DELETE',
  });

  if (!response.ok) {
    const errorText = await response.text();
    logger.error('Failed to delete slint asset:', {
      id,
      status: response.status,
      statusText: response.statusText,
      error: errorText,
    });
    throw new Error(`Failed to delete slint asset: ${errorText || response.statusText}`);
  }

  logger.info('Deleted slint asset:', id);
}

/**
 * Fetches the text content of a Slint asset file
 * @param scope - "system" or "user"
 * @param id - The asset filename
 */
export async function fetchSlintAssetContent(scope: string, id: string): Promise<string> {
  logger.info('Fetching slint asset content:', { scope, id });

  const response = await fetchApi(
    `/api/v1/assets/slint/file/${encodeURIComponent(scope)}/${encodeURIComponent(id)}`,
    {
      method: 'GET',
    }
  );

  if (!response.ok) {
    const errorText = await response.text();
    logger.error('Failed to fetch slint asset content:', {
      scope,
      id,
      status: response.status,
      statusText: response.statusText,
      error: errorText,
    });
    throw new Error(`Failed to fetch slint asset content: ${response.statusText}`);
  }

  return response.text();
}

// React Query hooks

/**
 * Hook to fetch slint assets with caching
 */
export function useSlintAssets() {
  return useQuery({
    queryKey: ['slintAssets'],
    queryFn: listSlintAssets,
    staleTime: 30000,
    refetchOnWindowFocus: true,
  });
}

/**
 * Hook to upload a slint asset
 */
export function useUploadSlintAsset() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: uploadSlintAsset,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['slintAssets'] });
    },
  });
}

/**
 * Hook to delete a slint asset
 */
export function useDeleteSlintAsset() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: deleteSlintAsset,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['slintAssets'] });
    },
  });
}
