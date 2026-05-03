// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { fetchApi } from './base';

export interface FrontendConfig {
  moqGatewayUrl?: string;
}

export async function fetchConfig(): Promise<FrontendConfig> {
  const response = await fetchApi('/api/v1/config');

  if (!response.ok) {
    throw new Error(`Failed to fetch config: ${response.statusText}`);
  }

  const data = await response.json();

  return {
    moqGatewayUrl: data.moq_gateway_url,
  };
}
