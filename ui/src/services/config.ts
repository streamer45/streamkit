// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { getApiUrl } from './base';

/**
 * Frontend configuration fetched from the server
 */
export interface FrontendConfig {
  moqGatewayUrl?: string;
}

/**
 * Fetch frontend configuration from the server
 */
export async function fetchConfig(): Promise<FrontendConfig> {
  const apiUrl = getApiUrl();
  const response = await fetch(`${apiUrl}/api/v1/config`);

  if (!response.ok) {
    throw new Error(`Failed to fetch config: ${response.statusText}`);
  }

  const data = await response.json();

  // Convert snake_case to camelCase
  return {
    moqGatewayUrl: data.moq_gateway_url,
  };
}
