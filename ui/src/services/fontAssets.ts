// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';

import type { FontAsset } from '@/types/generated/api-types';
import { getLogger } from '@/utils/logger';

import { fetchApi } from './base';

const logger = getLogger('fontAssets');

// ── Font loading for canvas preview ─────────────────────────────────────────

/** Set of font asset paths that have already been loaded into the browser. */
const loadedFonts = new Set<string>();

/** Derive a CSS font-family name from a font asset path (e.g. `"sk-Inter"`). */
export function fontFamilyForAsset(assetPath: string): string {
  const filename = assetPath.split('/').pop() ?? assetPath;
  const nameWithoutExt = filename.replace(/\.[^.]+$/, '');
  return `sk-${nameWithoutExt}`;
}

function fontServeUrl(assetPath: string): string {
  const parts = assetPath.split('/');
  const filename = parts.pop() ?? '';
  const scope = parts.pop() ?? 'system';
  return `/api/v1/assets/fonts/file/${encodeURIComponent(scope)}/${encodeURIComponent(filename)}`;
}

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

/** Load all font assets into the browser for canvas text overlay rendering. */
export async function loadFontAssets(assets: FontAsset[]): Promise<void> {
  await Promise.allSettled(assets.map(loadFontFace));
}

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
export function useFontAssets(enabled = true) {
  return useQuery({
    queryKey: ['fontAssets'],
    queryFn: listFontAssets,
    staleTime: 30_000,
    refetchOnWindowFocus: true,
    enabled,
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
