// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Generic service for plugin-declared asset types.
 *
 * All plugin asset types share the same REST endpoints parameterized by
 * `type_id`.  This module provides React Query hooks that work with any
 * plugin asset type.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';

import type { PluginAsset } from '@/types/generated/api-types';
import { getLogger } from '@/utils/logger';

import { fetchApi } from './base';

const logger = getLogger('pluginAssets');

/**
 * List all assets for a plugin-registered type.
 */
export async function listPluginAssets(typeId: string): Promise<PluginAsset[]> {
  logger.info('Fetching plugin assets:', typeId);

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
  logger.info('Fetched', assets.length, typeId, 'assets');

  return assets;
}

/**
 * Upload a file as a plugin asset.
 */
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

/**
 * Delete a user-uploaded plugin asset.
 */
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

/**
 * Fetch raw text content of a plugin asset (for editable text assets).
 */
export async function fetchPluginAssetContent(
  typeId: string,
  scope: string,
  id: string
): Promise<string> {
  const response = await fetchApi(
    `/api/v1/assets/plugin/${encodeURIComponent(typeId)}/file/${encodeURIComponent(scope)}/${encodeURIComponent(id)}`,
    { method: 'GET' }
  );

  if (!response.ok) {
    throw new Error(`Failed to fetch asset content: ${response.statusText}`);
  }

  return response.text();
}

/**
 * Update text content of a plugin asset in-place (user assets only).
 */
export async function updatePluginAssetContent(
  typeId: string,
  id: string,
  content: string
): Promise<PluginAsset> {
  const response = await fetchApi(
    `/api/v1/assets/plugin/${encodeURIComponent(typeId)}/file/user/${encodeURIComponent(id)}`,
    {
      method: 'PUT',
      headers: { 'Content-Type': 'text/plain' },
      body: content,
    }
  );

  if (!response.ok) {
    const errorText = await response.text();
    throw new Error(`Failed to update asset: ${errorText || response.statusText}`);
  }

  return response.json();
}

// ── React Query Hooks ────────────────────────────────────────────────────────

/**
 * Hook to fetch plugin assets of a specific type with caching.
 *
 * The query is only enabled when `typeId` is provided (non-empty).
 */
export function usePluginAssets(typeId: string) {
  return useQuery({
    queryKey: ['pluginAssets', typeId],
    queryFn: () => listPluginAssets(typeId),
    enabled: typeId.length > 0,
    staleTime: 30_000,
    refetchOnWindowFocus: true,
  });
}

/**
 * Hook to upload a plugin asset.
 */
export function useUploadPluginAsset(typeId: string) {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (file: File) => uploadPluginAsset(typeId, file),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['pluginAssets', typeId] });
    },
  });
}

/**
 * Hook to delete a plugin asset.
 */
export function useDeletePluginAsset(typeId: string) {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (id: string) => deletePluginAsset(typeId, id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['pluginAssets', typeId] });
    },
  });
}
