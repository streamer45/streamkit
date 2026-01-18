// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { fetchApi } from './base';

interface HealthResponse {
  status?: string;
  version?: string;
  build_hash?: string;
  buildHash?: string;
}

export interface HealthStatus {
  status: string;
  version: string;
  buildHash: string;
}

export async function fetchHealth(signal?: AbortSignal): Promise<HealthStatus> {
  const response = await fetchApi('/health', { signal });

  if (!response.ok) {
    throw new Error(`Failed to fetch health: ${response.statusText}`);
  }

  const data = (await response.json()) as HealthResponse;

  return {
    status: data.status ?? 'unknown',
    version: data.version ?? 'unknown',
    buildHash: data.build_hash ?? data.buildHash ?? 'unknown',
  };
}
