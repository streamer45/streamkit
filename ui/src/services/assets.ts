// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';

import type { AudioAsset } from '@/types/generated/api-types';
import { getLogger } from '@/utils/logger';

import { fetchApi } from './base';

const logger = getLogger('assets');

export async function listAudioAssets(): Promise<AudioAsset[]> {
  logger.info('Fetching audio assets');

  const response = await fetchApi('/api/v1/assets/audio', {
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

export async function uploadAudioAsset(file: File): Promise<AudioAsset> {
  logger.info('Uploading audio asset:', file.name);

  const formData = new FormData();
  formData.append('file', file);

  const response = await fetchApi('/api/v1/assets/audio', {
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

export async function deleteAudioAsset(id: string): Promise<void> {
  logger.info('Deleting audio asset:', id);

  const response = await fetchApi(`/api/v1/assets/audio/${encodeURIComponent(id)}`, {
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

export function useAudioAssets(enabled = true) {
  return useQuery({
    queryKey: ['audioAssets'],
    queryFn: listAudioAssets,
    staleTime: 30000, // Consider data fresh for 30 seconds
    refetchOnWindowFocus: true,
    enabled,
  });
}

export function useUploadAudioAsset() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: uploadAudioAsset,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['audioAssets'] });
    },
  });
}

export function useDeleteAudioAsset() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: deleteAudioAsset,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['audioAssets'] });
    },
  });
}
