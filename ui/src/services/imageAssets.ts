// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Service for managing image assets
 */

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
    headers: {
      'Content-Type': 'application/json',
    },
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
    const assets = await listImageAssets();
    const existing = assets.find((a) => a.id === file.name);
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
 * Deletes an image asset
 * @param id - The asset ID to delete
 * @returns A promise that resolves when the asset is deleted
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


