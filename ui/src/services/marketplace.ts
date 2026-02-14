// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type {
  InstallPluginRequest,
  InstallPluginResponse,
  JobInfo,
  MarketplaceIndex,
  MarketplacePluginDetails,
  MarketplaceRegistry,
} from '@/types/marketplace';

import { fetchApi } from './base';

export async function listMarketplaceRegistries(): Promise<MarketplaceRegistry[]> {
  const response = await fetchApi('/api/v1/marketplace/registries');

  if (!response.ok) {
    const text = await response.text();
    throw new Error(text || `Failed to fetch registries (status ${response.status})`);
  }

  return response.json() as Promise<MarketplaceRegistry[]>;
}

export async function listMarketplacePlugins(
  registry: string,
  query?: string
): Promise<MarketplaceIndex> {
  const params = new URLSearchParams({ registry });
  if (query && query.trim()) {
    params.set('q', query.trim());
  }

  const response = await fetchApi(`/api/v1/marketplace/plugins?${params.toString()}`);

  if (!response.ok) {
    const text = await response.text();
    throw new Error(text || `Failed to fetch marketplace plugins (status ${response.status})`);
  }

  return response.json() as Promise<MarketplaceIndex>;
}

export async function getMarketplacePlugin(
  registry: string,
  pluginId: string,
  version?: string
): Promise<MarketplacePluginDetails> {
  const params = new URLSearchParams({ registry });
  if (version && version.trim()) {
    params.set('version', version.trim());
  }

  const response = await fetchApi(
    `/api/v1/marketplace/plugins/${encodeURIComponent(pluginId)}?${params.toString()}`
  );

  if (!response.ok) {
    const text = await response.text();
    throw new Error(text || `Failed to fetch plugin details (status ${response.status})`);
  }

  return response.json() as Promise<MarketplacePluginDetails>;
}

export async function installMarketplacePlugin(
  request: InstallPluginRequest
): Promise<InstallPluginResponse> {
  const response = await fetchApi('/api/v1/plugins/install', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(request),
  });

  if (!response.ok) {
    const text = await response.text();
    throw new Error(text || `Failed to install plugin (status ${response.status})`);
  }

  return response.json() as Promise<InstallPluginResponse>;
}

export async function getMarketplaceJob(jobId: string): Promise<JobInfo> {
  const response = await fetchApi(`/api/v1/jobs/${encodeURIComponent(jobId)}`);

  if (!response.ok) {
    const text = await response.text();
    throw new Error(text || `Failed to fetch job (status ${response.status})`);
  }

  return response.json() as Promise<JobInfo>;
}

export async function cancelMarketplaceJob(jobId: string): Promise<JobInfo> {
  const response = await fetchApi(`/api/v1/jobs/${encodeURIComponent(jobId)}/cancel`, {
    method: 'POST',
  });

  if (!response.ok) {
    const text = await response.text();
    throw new Error(text || `Failed to cancel job (status ${response.status})`);
  }

  return response.json() as Promise<JobInfo>;
}
