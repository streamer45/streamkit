// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { PluginSummary } from '@/types/types';

import { fetchApi } from './base';

export async function uploadPlugin(file: File): Promise<PluginSummary> {
  const formData = new FormData();
  formData.append('plugin', file, file.name);

  const response = await fetchApi('/api/v1/plugins', {
    method: 'POST',
    body: formData,
  });

  if (!response.ok) {
    const text = await response.text();
    throw new Error(text || `Failed to upload plugin (status ${response.status})`);
  }

  return response.json() as Promise<PluginSummary>;
}

export async function deletePlugin(
  kind: string,
  options?: { keepFile?: boolean }
): Promise<PluginSummary> {
  const query = options?.keepFile ? '?keep_file=true' : '';
  const response = await fetchApi(`/api/v1/plugins/${encodeURIComponent(kind)}${query}`, {
    method: 'DELETE',
  });

  if (!response.ok) {
    const text = await response.text();
    throw new Error(text || `Failed to remove plugin (status ${response.status})`);
  }

  return response.json() as Promise<PluginSummary>;
}
