// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Service for managing font assets and loading them into the browser
 * via the CSS Font Loading API for accurate canvas previews.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';

import type { FontAsset } from '@/types/generated/api-types';
import { getLogger } from '@/utils/logger';

import { fetchApi } from './base';

const logger = getLogger('fontAssets');

// ── Font loading for canvas preview ─────────────────────────────────────────

/** Set of font asset paths that have already been loaded into the browser. */
const loadedFonts = new Set<string>();

/**
 * Derive a unique CSS font-family name from a font asset path.
 *
 * E.g. `"samples/fonts/system/Inter.ttf"` → `"sk-Inter"`.
 * The `sk-` prefix avoids collisions with system fonts.
 */
export function fontFamilyForAsset(assetPath: string): string {
  const filename = assetPath.split('/').pop() ?? assetPath;
  const nameWithoutExt = filename.replace(/\.[^.]+$/, '');
  return `sk-${nameWithoutExt}`;
}

/**
 * Build the serve URL for a font asset.
 *
 * E.g. `"samples/fonts/system/Inter.ttf"` →
 *      `"/api/v1/assets/fonts/file/system/Inter.ttf"`.
 */
function fontServeUrl(assetPath: string): string {
  const parts = assetPath.split('/');
  const filename = parts.pop() ?? '';
  const scope = parts.pop() ?? 'system';
  return `/api/v1/assets/fonts/file/${encodeURIComponent(scope)}/${encodeURIComponent(filename)}`;
}

/**
 * Load a single font asset into the browser using the CSS Font Loading API.
 *
 * Once loaded, text rendered with `font-family: fontFamilyForAsset(path)`
 * will use the actual font file from the server.  No-ops if the font has
 * already been loaded.
 */
async function loadFontFace(asset: FontAsset): Promise<void> {
  if (loadedFonts.has(asset.path)) return;

  const family = fontFamilyForAsset(asset.path);
  const url = fontServeUrl(asset.path);
  const isBold = asset.id.includes('-Bold') || asset.id.includes('Bold');

  try {
    const face = new FontFace(family, `url(${url})`, {
      weight: isBold ? '700' : '400',
      style: 'normal',
    });
    await face.load();
    document.fonts.add(face);
    loadedFonts.add(asset.path);
  } catch (e) {
    logger.warn(`Failed to load font '${asset.name}':`, e);
  }
}

/**
 * Load all font assets into the browser for canvas preview rendering.
 *
 * Call this after fetching the font asset list so that compositor canvas
 * text overlays render with the actual server-side font rather than a
 * generic CSS fallback.
 */
export async function loadFontAssets(assets: FontAsset[]): Promise<void> {
  await Promise.allSettled(assets.map(loadFontFace));
}

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

// ── React Query hooks ───────────────────────────────────────────────────────

/**
 * Hook to fetch font assets with caching
 */
export function useFontAssets() {
  return useQuery({
    queryKey: ['fontAssets'],
    queryFn: listFontAssets,
    staleTime: 30_000,
    refetchOnWindowFocus: true,
  });
}

/**
 * Hook to upload a font asset
 */
export function useUploadFontAsset() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: uploadFontAsset,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['fontAssets'] });
    },
  });
}

/**
 * Hook to delete a font asset
 */
export function useDeleteFontAsset() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: deleteFontAsset,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['fontAssets'] });
    },
  });
}
