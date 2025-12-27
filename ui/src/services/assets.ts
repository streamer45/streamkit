// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Service for managing audio assets
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';

import type { AudioAsset } from '@/types/generated/api-types';
import { getLogger } from '@/utils/logger';

import { getApiUrl } from './base';

const logger = getLogger('assets');

/**
 * Lists all available audio assets (system + user)
 * @returns A promise that resolves to an array of audio assets
 */
export async function listAudioAssets(): Promise<AudioAsset[]> {
  const apiUrl = getApiUrl();
  const endpoint = `${apiUrl}/api/v1/assets/audio`;

  logger.info('Fetching audio assets from:', endpoint);

  const response = await fetch(endpoint, {
    method: 'GET',
    headers: {
      'Content-Type': 'application/json',
    },
  });

  if (!response.ok) {
    const errorText = await response.text();
    logger.error('Failed to fetch audio assets:', {
      status: response.status,
      statusText: response.statusText,
      error: errorText,
    });
    throw new Error(`Failed to fetch audio assets: ${response.statusText}`);
  }

  const assets: AudioAsset[] = await response.json();
  logger.info('Fetched', assets.length, 'audio assets');

  return assets;
}

/**
 * Uploads a new audio asset
 * @param file - The audio file to upload
 * @returns A promise that resolves to the created audio asset
 */
export async function uploadAudioAsset(file: File): Promise<AudioAsset> {
  const apiUrl = getApiUrl();
  const endpoint = `${apiUrl}/api/v1/assets/audio`;

  logger.info('Uploading audio asset:', file.name);

  const formData = new FormData();
  formData.append('file', file);

  const response = await fetch(endpoint, {
    method: 'POST',
    body: formData,
  });

  if (!response.ok) {
    const errorText = await response.text();
    logger.error('Failed to upload audio asset:', {
      fileName: file.name,
      status: response.status,
      statusText: response.statusText,
      error: errorText,
    });
    throw new Error(`Failed to upload audio asset: ${errorText || response.statusText}`);
  }

  const asset: AudioAsset = await response.json();
  logger.info('Uploaded audio asset:', asset.name);

  return asset;
}

/**
 * Deletes an audio asset
 * @param id - The asset ID to delete
 * @returns A promise that resolves when the asset is deleted
 */
export async function deleteAudioAsset(id: string): Promise<void> {
  const apiUrl = getApiUrl();
  const endpoint = `${apiUrl}/api/v1/assets/audio/${encodeURIComponent(id)}`;

  logger.info('Deleting audio asset:', id);

  const response = await fetch(endpoint, {
    method: 'DELETE',
  });

  if (!response.ok) {
    const errorText = await response.text();
    logger.error('Failed to delete audio asset:', {
      id,
      status: response.status,
      statusText: response.statusText,
      error: errorText,
    });
    throw new Error(`Failed to delete audio asset: ${errorText || response.statusText}`);
  }

  logger.info('Deleted audio asset:', id);
}

// React Query hooks

/**
 * Hook to fetch audio assets with caching
 */
export function useAudioAssets() {
  return useQuery({
    queryKey: ['audioAssets'],
    queryFn: listAudioAssets,
    staleTime: 30000, // Consider data fresh for 30 seconds
    refetchOnWindowFocus: true,
  });
}

/**
 * Hook to upload an audio asset
 */
export function useUploadAudioAsset() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: uploadAudioAsset,
    onSuccess: () => {
      // Invalidate and refetch audio assets list
      queryClient.invalidateQueries({ queryKey: ['audioAssets'] });
    },
  });
}

/**
 * Hook to delete an audio asset
 */
export function useDeleteAudioAsset() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: deleteAudioAsset,
    onSuccess: () => {
      // Invalidate and refetch audio assets list
      queryClient.invalidateQueries({ queryKey: ['audioAssets'] });
    },
  });
}
