// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Service for managing image assets
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';

import type { ImageAsset } from '@/types/generated/api-types';
import { getLogger } from '@/utils/logger';

import { fetchApi } from './base';

const logger = getLogger('imageAssets');

/**
 * Lists all available image assets (system + user)
 * @returns A promise that resolves to an array of image assets
 */
export async function listImageAssets(): Promise<ImageAsset[]> {
  logger.info('Fetching image assets');

  const response = await fetchApi('/api/v1/assets/images', {
    method: 'GET',
  });

  if (!response.ok) {
    const errorText = await response.text();
    logger.error('Failed to fetch image assets:', {
      status: response.status,
      statusText: response.statusText,
      error: errorText,
    });
    throw new Error(`Failed to fetch image assets: ${response.statusText}`);
  }

  const assets: ImageAsset[] = await response.json();
  logger.info('Fetched', assets.length, 'image assets');

  return assets;
}

/**
 * Uploads a new image asset
 * @param file - The image file to upload
 * @returns A promise that resolves to the created image asset
 */
export async function uploadImageAsset(file: File): Promise<ImageAsset> {
  logger.info('Uploading image asset:', file.name);

  const formData = new FormData();
  formData.append('file', file);

  const response = await fetchApi('/api/v1/assets/images', {
    method: 'POST',
    body: formData,
  });

  if (response.status === 409) {
    // Conflict — file already exists. Fetch existing assets to find the match.
    // The server sanitizes filenames (spaces → underscores, etc.), so match
    // against the sanitized name rather than the raw file.name.
    const sanitized = file.name.replace(/[^a-zA-Z0-9._-]/g, '_');
    const assets = await listImageAssets();
    const existing = assets.find((a) => a.id === sanitized);
    if (existing) {
      logger.info('Image asset already exists, reusing:', existing.path);
      return existing;
    }
    throw new Error(`Image asset already exists: ${file.name}`);
  }

  if (!response.ok) {
    const errorText = await response.text();
    logger.error('Failed to upload image asset:', {
      fileName: file.name,
      status: response.status,
      statusText: response.statusText,
      error: errorText,
    });
    throw new Error(`Failed to upload image asset: ${errorText || response.statusText}`);
  }

  const asset: ImageAsset = await response.json();
  logger.info('Uploaded image asset:', asset.name);

  return asset;
}

/**
 * Deletes an image asset by ID
 * @param id - The image asset ID to delete
 */
export async function deleteImageAsset(id: string): Promise<void> {
  logger.info('Deleting image asset:', id);

  const response = await fetchApi(`/api/v1/assets/images/${encodeURIComponent(id)}`, {
    method: 'DELETE',
  });

  if (!response.ok) {
    const errorText = await response.text();
    logger.error('Failed to delete image asset:', {
      id,
      status: response.status,
      statusText: response.statusText,
      error: errorText,
    });
    throw new Error(`Failed to delete image asset: ${errorText || response.statusText}`);
  }

  logger.info('Deleted image asset:', id);
}

// React Query hooks

/**
 * Hook to fetch image assets with caching
 */
export function useImageAssets() {
  return useQuery({
    queryKey: ['imageAssets'],
    queryFn: listImageAssets,
    staleTime: 30000,
    refetchOnWindowFocus: true,
  });
}

/**
 * Hook to upload an image asset
 */
export function useUploadImageAsset() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: uploadImageAsset,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['imageAssets'] });
    },
  });
}

/**
 * Hook to delete an image asset
 */
export function useDeleteImageAsset() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: deleteImageAsset,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['imageAssets'] });
    },
  });
}
