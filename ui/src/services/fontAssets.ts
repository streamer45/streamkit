// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Service for managing font assets
 */

import type { FontAsset } from '@/types/generated/api-types';
import { getLogger } from '@/utils/logger';

import { fetchApi } from './base';

const logger = getLogger('fontAssets');

/**
 * Lists all available font assets (system + user)
 * @returns A promise that resolves to an array of font assets
 */
export async function listFontAssets(): Promise<FontAsset[]> {
  logger.info('Fetching font assets');

  const response = await fetchApi('/api/v1/assets/fonts', {
    method: 'GET',
  });

  if (!response.ok) {
    const errorText = await response.text();
    logger.error('Failed to fetch font assets:', {
      status: response.status,
      statusText: response.statusText,
      error: errorText,
    });
    throw new Error(`Failed to fetch font assets: ${response.statusText}`);
  }

  const assets: FontAsset[] = await response.json();
  logger.info('Fetched', assets.length, 'font assets');

  return assets;
}

/**
 * Uploads a new font asset
 * @public
 * @param file - The font file to upload
 * @returns A promise that resolves to the created font asset
 */
export async function uploadFontAsset(file: File): Promise<FontAsset> {
  logger.info('Uploading font asset:', file.name);

  const formData = new FormData();
  formData.append('file', file);

  const response = await fetchApi('/api/v1/assets/fonts', {
    method: 'POST',
    body: formData,
  });

  if (response.status === 409) {
    const sanitized = file.name.replace(/[^a-zA-Z0-9._-]/g, '_');
    const assets = await listFontAssets();
    const existing = assets.find((a) => a.id === sanitized);
    if (existing) {
      logger.info('Font asset already exists, reusing:', existing.path);
      return existing;
    }
    throw new Error(`Font asset already exists: ${file.name}`);
  }

  if (!response.ok) {
    const errorText = await response.text();
    logger.error('Failed to upload font asset:', {
      fileName: file.name,
      status: response.status,
      statusText: response.statusText,
      error: errorText,
    });
    throw new Error(`Failed to upload font asset: ${errorText || response.statusText}`);
  }

  const asset: FontAsset = await response.json();
  logger.info('Uploaded font asset:', asset.name);

  return asset;
}

/**
 * Deletes a font asset by ID
 * @public
 * @param id - The font asset ID to delete
 */
export async function deleteFontAsset(id: string): Promise<void> {
  logger.info('Deleting font asset:', id);

  const response = await fetchApi(`/api/v1/assets/fonts/${encodeURIComponent(id)}`, {
    method: 'DELETE',
  });

  if (!response.ok) {
    const errorText = await response.text();
    logger.error('Failed to delete font asset:', {
      id,
      status: response.status,
      statusText: response.statusText,
      error: errorText,
    });
    throw new Error(`Failed to delete font asset: ${errorText || response.statusText}`);
  }

  logger.info('Deleted font asset:', id);
}
